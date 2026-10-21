//! A session owns immutable source snapshots and the canonical reference index.

use crate::schema_registry::SchemaSelection;
use crate::{
    Address, Diagnostic, Dialect, Location, Options, ParserError, ReferenceEdge, Result,
    SchemaDialect, Severity, SourceDocument,
};
use crate::{
    pointer, retrieval, source,
    syntax::{self, Context},
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// Host I/O is deliberately separate from syntax and reference semantics.
/// Implementations must enforce byte limits before returning content.
#[derive(Debug, Clone, Copy)]
pub struct ReadContext<'a> {
    pub policy: &'a crate::ResolveOptions,
    pub max_bytes: usize,
}

pub trait Resolver {
    fn read(&mut self, uri: &str, context: ReadContext<'_>) -> Result<String>;

    /// Return the final checked retrieval URI after redirects. Existing custom
    /// resolvers retain their original behavior through this default method.
    fn read_with_identity(
        &mut self,
        uri: &str,
        context: ReadContext<'_>,
    ) -> Result<ResolvedSource> {
        self.read(uri, context).map(|content| ResolvedSource {
            content,
            retrieval_uri: uri.into(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedSource {
    pub content: String,
    pub retrieval_uri: String,
}

#[derive(Debug, Default)]
pub struct NoIo;
impl Resolver for NoIo {
    fn read(&mut self, uri: &str, _: ReadContext<'_>) -> Result<String> {
        Err(ParserError::new(
            "RESOLVER_DENIED",
            "Supply virtual files or an explicit host resolver",
            Address {
                uri: uri.into(),
                pointer: String::new(),
            },
        ))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NodeContext {
    pub context: Context,
    pub schema_dialect: SchemaDialect,
    pub api_dialect: Dialect,
    // Most nodes inherit one scope. Share immutable resource metadata instead
    // of allocating the same URI and root address for every syntax container.
    pub base: Arc<str>,
    pub resource: Arc<Address>,
}

pub(crate) struct Session<'a> {
    pub root_uri: String,
    pub dialect: Dialect,
    pub options: &'a Options,
    pub sources: BTreeMap<String, SourceDocument>,
    working_root: Option<Value>,
    capture_locations: bool,
    pub contexts: BTreeMap<Address, NodeContext>,
    pub references: Vec<ReferenceEdge>,
    reference_index: HashMap<Address, [Option<usize>; 3]>,
    reference_nodes: BTreeSet<Address>,
    pub diagnostics: Vec<Diagnostic>,
    resources: BTreeMap<String, Address>,
    anchors: BTreeMap<String, Address>,
    files: BTreeMap<String, String>,
    retrieval_aliases: BTreeMap<String, String>,
    total_bytes: usize,
    total_nodes: usize,
    total_value_bytes: usize,
    retrievals: usize,
    /// Foreign documents are inert except for explicitly declared schema roots
    /// and schema reference targets discovered from those roots.
    pub schema_sources: BTreeSet<String>,
    pub schema_mode: bool,
    schema_defaults: BTreeMap<Address, SchemaDialect>,
    schema_api_defaults: BTreeMap<String, Dialect>,
    schema_selection: Option<SchemaSelection>,
    resolver: &'a mut dyn Resolver,
}

impl<'a> Session<'a> {
    pub fn new(
        root_uri: String,
        options: &'a Options,
        resolver: &'a mut dyn Resolver,
        capture_locations: bool,
    ) -> Result<Self> {
        options.limits.check()?;
        let at = Address {
            uri: root_uri.clone(),
            pointer: String::new(),
        };
        let root = pointer::checked_url(&root_uri, &root_uri, &at)?;
        if root.fragment().is_some() {
            return Err(ParserError::new(
                "INVALID_URI",
                "Input retrieval URI must not contain a fragment",
                at,
            ));
        }
        let retrieval_aliases = retrieval::normalize(
            &options.retrieval_aliases,
            root.as_str(),
            options.limits.max_documents,
        )?;
        let mut files = BTreeMap::new();
        for (uri, content) in &options.files {
            let url = pointer::checked_url(uri, &root_uri, &at)?;
            if url.fragment().is_some() {
                return Err(ParserError::new(
                    "INVALID_OPTIONS",
                    "Virtual file keys cannot contain fragments",
                    at,
                ));
            }
            if retrieval_aliases.contains_key(url.as_str()) {
                return Err(ParserError::new(
                    "INVALID_OPTIONS",
                    "Virtual files must use final retrieval URI keys",
                    at,
                ));
            }
            if files.insert(url.to_string(), content.clone()).is_some() {
                return Err(ParserError::new(
                    "DUPLICATE_RESOURCE",
                    "Virtual file keys normalize to the same URI",
                    at,
                ));
            }
        }
        Ok(Self {
            root_uri: retrieval::canonical(&retrieval_aliases, root.as_str()).into(),
            dialect: Dialect::OpenApi31,
            options,
            resolver,
            sources: BTreeMap::new(),
            working_root: None,
            capture_locations,
            contexts: BTreeMap::new(),
            references: Vec::new(),
            reference_index: HashMap::new(),
            reference_nodes: BTreeSet::new(),
            diagnostics: Vec::new(),
            resources: BTreeMap::new(),
            anchors: BTreeMap::new(),
            files,
            retrieval_aliases,
            total_bytes: 0,
            total_nodes: 0,
            total_value_bytes: 0,
            retrievals: 0,
            schema_sources: BTreeSet::new(),
            schema_mode: false,
            schema_defaults: BTreeMap::new(),
            schema_api_defaults: BTreeMap::new(),
            schema_selection: None,
        })
    }

    pub fn initialize(
        &mut self,
        content: Option<String>,
        generated: bool,
        normalize: bool,
    ) -> Result<()> {
        let mut uri = self.root_uri.clone();
        let text = match content {
            Some(text) => {
                self.retrievals = 1;
                text
            }
            None => {
                let source = self.read(&uri)?;
                uri = source.retrieval_uri;
                self.root_uri = uri.clone();
                source.content
            }
        };
        let source = self.parse_source(&uri, text, generated)?;
        self.dialect = source::detect(&source.document, &uri)?;
        if normalize && crate::defaults::needs_info_version(&source.document) {
            let mut working = source.document.clone();
            crate::defaults::info_version(&mut working);
            self.working_root = Some(working);
        }
        self.add(source, None)
    }

    fn parse_source(&self, uri: &str, text: String, generated: bool) -> Result<SourceDocument> {
        if !generated && uri == self.root_uri {
            return source::read_with_format(
                uri,
                text,
                &self.options.limits,
                self.capture_locations,
                self.options.root_format,
            );
        }
        if self.schema_sources.contains(uri)
            && text
                .trim_start_matches('\u{feff}')
                .trim_start()
                .starts_with("#%RAML")
        {
            return Ok(source::parse_raml(uri, text, &self.options.limits)?.source);
        }
        match (generated, self.capture_locations) {
            (true, true) => source::parse_generated(uri, text, &self.options.limits),
            (true, false) => source::parse_generated_document(uri, text, &self.options.limits),
            (false, true) => source::parse(uri, text, &self.options.limits),
            (false, false) => source::parse_document(uri, text, &self.options.limits),
        }
    }

    /// Document-only callers pay for text positions only on failure. The stored
    /// text is immutable, so replaying the same bounded loader cannot change values.
    pub fn restore_diagnostic_locations(&mut self) -> Result<()> {
        let uris: BTreeSet<_> = self
            .diagnostics
            .iter()
            .flat_map(|diagnostic| std::iter::once(&diagnostic.location).chain(&diagnostic.related))
            .map(|location| location.address.uri.clone())
            .collect();
        for uri in uris {
            if let Some(source) = self.sources.get_mut(&uri)
                && !source.generated
            {
                let format = if uri == self.root_uri {
                    self.options.root_format
                } else {
                    crate::SourceFormat::Auto
                };
                let full = source::read_with_format(
                    &uri,
                    source.text.clone(),
                    &self.options.limits,
                    true,
                    format,
                )?;
                source.locations = full.locations;
            }
        }
        let mut diagnostics = std::mem::take(&mut self.diagnostics);
        for diagnostic in &mut diagnostics {
            if diagnostic.location.line.is_none() {
                diagnostic.location = self.locate(&diagnostic.location.address);
            }
            for related in &mut diagnostic.related {
                if related.line.is_none() {
                    *related = self.locate(&related.address);
                }
            }
        }
        self.diagnostics = diagnostics;
        Ok(())
    }

    fn read(&mut self, requested_uri: &str) -> Result<ResolvedSource> {
        let uri = retrieval::canonical(&self.retrieval_aliases, requested_uri).to_owned();
        let uri = uri.as_str();
        let at = Address {
            uri: uri.into(),
            pointer: String::new(),
        };
        if self.retrievals >= self.options.limits.max_documents {
            return Err(ParserError::new(
                "LIMIT_DOCUMENTS",
                "Document retrieval budget exhausted",
                at,
            ));
        }
        self.retrievals += 1;
        let remaining = self
            .options
            .limits
            .max_total_bytes
            .saturating_sub(self.total_bytes);
        if remaining == 0 {
            return Err(ParserError::new(
                "LIMIT_TOTAL_BYTES",
                "Total source byte budget exhausted",
                at,
            ));
        }
        let source = if let Some(content) = self.files.get(uri) {
            ResolvedSource {
                content: content.clone(),
                retrieval_uri: uri.into(),
            }
        } else {
            self.resolver.read_with_identity(
                uri,
                ReadContext {
                    policy: &self.options.resolve,
                    max_bytes: self.options.limits.max_document_bytes.min(remaining),
                },
            )?
        };
        if source.content.len() > remaining {
            return Err(ParserError::new(
                "LIMIT_TOTAL_BYTES",
                "Total source byte budget exhausted",
                at,
            ));
        }
        let final_uri = retrieval::checked_uri(&source.retrieval_uri, uri, &at)?;
        if final_uri != uri {
            self.register_retrieval_alias(uri, &final_uri)?;
        }
        let final_uri = retrieval::canonical(&self.retrieval_aliases, &final_uri).to_owned();
        if self.schema_sources.contains(uri) {
            self.schema_sources.insert(final_uri.clone());
        }
        if let Some(existing) = self.sources.get(&final_uri)
            && existing.text != source.content
        {
            return Err(ParserError::new(
                "DUPLICATE_RESOURCE",
                "Retrieval returned conflicting content for an existing source",
                at,
            ));
        }
        Ok(ResolvedSource {
            content: source.content,
            retrieval_uri: final_uri,
        })
    }

    fn register_retrieval_alias(&mut self, requested: &str, final_uri: &str) -> Result<()> {
        let at = Address {
            uri: requested.into(),
            pointer: String::new(),
        };
        if self.retrieval_aliases.len() >= self.options.limits.max_documents {
            return Err(ParserError::new(
                "LIMIT_DOCUMENTS",
                "Retrieval alias budget exhausted",
                at,
            ));
        }
        let mut aliases = self.retrieval_aliases.clone();
        aliases.insert(requested.into(), final_uri.into());
        let aliases = retrieval::flatten(&aliases, &at)?;
        for (alias, target) in &aliases {
            if self.sources.contains_key(alias)
                || self
                    .resources
                    .get(alias)
                    .is_some_and(|address| address.uri != *target || !address.pointer.is_empty())
            {
                return Err(ParserError::new(
                    "DUPLICATE_RESOURCE",
                    "Retrieval alias conflicts with an existing source or schema resource",
                    at,
                ));
            }
        }
        self.retrieval_aliases = aliases;
        Ok(())
    }

    fn add(&mut self, source: SourceDocument, context: Option<Context>) -> Result<()> {
        if self.sources.contains_key(&source.uri) {
            return Ok(());
        }
        if self.sources.len() >= self.options.limits.max_documents {
            return Err(ParserError::new(
                "LIMIT_DOCUMENTS",
                "Document count limit exceeded",
                Address {
                    uri: source.uri.clone(),
                    pointer: String::new(),
                },
            ));
        }
        self.total_bytes = self.total_bytes.saturating_add(source.bytes);
        if self.total_bytes > self.options.limits.max_total_bytes {
            return Err(ParserError::new(
                "LIMIT_TOTAL_BYTES",
                "Total source byte limit exceeded",
                Address {
                    uri: source.uri.clone(),
                    pointer: String::new(),
                },
            ));
        }
        let expanded_bytes = crate::output::measure_json(
            &source.document,
            self.options
                .limits
                .max_total_bytes
                .saturating_sub(self.total_value_bytes),
        )
        .map_err(|_| {
            ParserError::new(
                "LIMIT_TOTAL_BYTES",
                "Expanded source values exceed total byte budget",
                Address {
                    uri: source.uri.clone(),
                    pointer: String::new(),
                },
            )
        })?;
        self.total_value_bytes += expanded_bytes;
        self.total_nodes = self
            .total_nodes
            .saturating_add(crate::source::value_size(&source.document).0);
        if self.total_nodes > self.options.limits.max_nodes {
            return Err(ParserError::new(
                "LIMIT_NODES",
                "Total source node budget exhausted",
                Address {
                    uri: source.uri.clone(),
                    pointer: String::new(),
                },
            ));
        }
        let uri = source.uri.clone();
        let root = Address {
            uri: uri.clone(),
            pointer: String::new(),
        };
        self.insert_resource(uri.clone(), root.clone())?;
        self.index_nodes(
            &uri,
            "",
            context.unwrap_or_else(|| syntax::root_context(&source.document)),
            Some(source),
        )
    }

    /// Register only the schema subtree, preserving the containing source and
    /// its retrieval URI as the default JSON Schema resource boundary.
    pub fn add_schema_root(&mut self, address: &Address) -> Result<()> {
        let url = pointer::checked_url(&address.uri, &self.root_uri, address)?;
        if url.fragment().is_some()
            || !address.pointer.is_empty() && !address.pointer.starts_with('/')
        {
            return Err(ParserError::new(
                "INVALID_SCHEMA_ROOT",
                "Schema roots require a fragment-free URI and JSON Pointer",
                address.clone(),
            ));
        }
        let mut uri = retrieval::canonical(&self.retrieval_aliases, url.as_str()).to_owned();
        self.schema_sources.insert(uri.clone());
        if !self.sources.contains_key(&uri) {
            let retrieved = self.read(&uri)?;
            uri = retrieved.retrieval_uri;
            let source = self.parse_source(&uri, retrieved.content, false)?;
            self.add(source, Some(Context::Literal))?;
        }
        let root = Address {
            uri: uri.clone(),
            pointer: address.pointer.clone(),
        };
        if self
            .contexts
            .get(&root)
            .is_some_and(|node| node.context == Context::Schema)
        {
            return Ok(());
        }
        if self
            .value(&root)
            .is_none_or(|value| !value.is_object() && !value.is_boolean())
        {
            return Err(ParserError::new(
                "INVALID_SCHEMA_ROOT",
                "Schema root must identify an object or boolean schema",
                root,
            ));
        }
        self.index_nodes(&uri, &root.pointer, Context::Schema, None)
    }

    pub fn set_schema_default(&mut self, address: &Address, dialect: SchemaDialect) -> Result<()> {
        let url = pointer::checked_url(&address.uri, &self.root_uri, address)?;
        let address = Address {
            uri: retrieval::canonical(&self.retrieval_aliases, url.as_str()).into(),
            pointer: address.pointer.clone(),
        };
        if self
            .schema_defaults
            .get(&address)
            .is_some_and(|previous| *previous != dialect)
        {
            return Err(ParserError::new(
                "SCHEMA_DIALECT_CONFLICT",
                "The same physical schema has conflicting default dialects",
                address,
            ));
        }
        self.schema_defaults.insert(address, dialect);
        Ok(())
    }

    fn schema_default(&self, uri: &str, start: &str) -> SchemaDialect {
        let openapi_owner = self
            .sources
            .get(uri)
            .is_some_and(|source| source.document.get("openapi").is_some());
        let mut pointer = start;
        loop {
            let address = Address {
                uri: uri.into(),
                pointer: pointer.into(),
            };
            if !openapi_owner && let Some(dialect) = self.schema_defaults.get(&address) {
                return *dialect;
            }
            if let Some(context) = self.contexts.get(&address) {
                return context.schema_dialect;
            }
            match pointer.rsplit_once('/') {
                Some((parent, _)) => pointer = parent,
                None => break,
            }
        }
        self.sources
            .get(uri)
            .and_then(|source| source.document.get("jsonSchemaDialect"))
            .and_then(Value::as_str)
            .and_then(SchemaDialect::from_uri)
            .unwrap_or_default()
    }

    pub(crate) fn schema_uri(&self, root: &Address) -> Option<&str> {
        let mut pointer = root.pointer.as_str();
        loop {
            let address = Address {
                uri: root.uri.clone(),
                pointer: pointer.into(),
            };
            if self
                .contexts
                .get(&address)
                .is_some_and(|context| context.context == Context::Schema)
                && let Some(uri) = self
                    .value(&address)
                    .and_then(|value| value.get("$schema"))
                    .and_then(Value::as_str)
            {
                return Some(uri);
            }
            match pointer.rsplit_once('/') {
                Some((parent, _)) => pointer = parent,
                None => break,
            }
        }
        self.sources
            .get(&root.uri)
            .and_then(|source| source.document.get("jsonSchemaDialect"))
            .and_then(Value::as_str)
    }

    fn index_nodes(
        &mut self,
        uri: &str,
        start: &str,
        context: Context,
        source: Option<SourceDocument>,
    ) -> Result<()> {
        let mut nodes = Vec::new();
        let root = Address {
            uri: uri.into(),
            pointer: String::new(),
        };
        let document = if uri == self.root_uri {
            self.working_root
                .as_ref()
                .or_else(|| source.as_ref().map(|source| &source.document))
        } else {
            source.as_ref().map(|source| &source.document)
        }
        .or_else(|| self.sources.get(uri).map(|source| &source.document))
        .and_then(|document| document.pointer(start))
        .ok_or_else(|| {
            ParserError::new(
                "INVALID_SCHEMA_ROOT",
                "Schema subtree is missing",
                Address {
                    uri: uri.into(),
                    pointer: start.into(),
                },
            )
        })?;
        // A foreign API owns its Schema Object semantics. The generated modern
        // envelope must not activate identifiers or reference siblings that
        // OpenAPI 3.0 ignores. Plain external schemas inherit their referrer;
        // an explicit JSON Schema resource starts a modern schema boundary.
        let owner = source.as_ref().or_else(|| self.sources.get(uri));
        let mut boundary = start;
        let declared_resource = owner.and_then(|source| {
            loop {
                if let Some(value) = source
                    .document
                    .pointer(boundary)
                    .and_then(|value| value.get("$schema"))
                {
                    break Some(value);
                }
                match boundary.rsplit_once('/') {
                    Some((parent, _)) => boundary = parent,
                    None => break None,
                }
            }
        });
        let api_dialect = if let Some(owner) = owner.filter(|source| {
            source.document.get("openapi").is_some() || source.document.get("swagger").is_some()
        }) {
            source::detect(&owner.document, uri)?
        } else if declared_resource.is_some() {
            Dialect::OpenApi31
        } else {
            self.schema_api_defaults
                .get(uri)
                .copied()
                .unwrap_or(self.dialect)
        };
        let schema_default = if !api_dialect.modern_schema() {
            SchemaDialect::Draft4
        } else if let Some(dialect) = declared_resource
            .and_then(Value::as_str)
            .and_then(SchemaDialect::from_uri)
        {
            dialect
        } else if let Some(source) = source
            .as_ref()
            .filter(|source| source.document.get("openapi").is_some())
        {
            // An imported OpenAPI document owns its default even before its
            // snapshot has been inserted into the session's source registry.
            source
                .document
                .get("jsonSchemaDialect")
                .and_then(Value::as_str)
                .and_then(SchemaDialect::from_uri)
                .unwrap_or_default()
        } else {
            source
                .as_ref()
                .and_then(|source| source.document.get("jsonSchemaDialect"))
                .and_then(Value::as_str)
                .and_then(SchemaDialect::from_uri)
                .unwrap_or_else(|| self.schema_default(uri, start))
        };
        syntax::visit_schema(
            document,
            start,
            context,
            api_dialect,
            schema_default,
            &mut |value, pointer, context, schema_dialect| {
                if value.is_object()
                    || value.is_array()
                    || context == Context::Schema && value.is_boolean()
                {
                    let metadata = if api_dialect.modern_schema()
                        && context == Context::Schema
                        && !(schema_dialect.ignores_reference_siblings()
                            && value.get("$ref").is_some())
                    {
                        Value::Object(
                            std::iter::once(schema_dialect.identifier())
                                .chain(syntax::anchor_keywords(schema_dialect).iter().copied())
                                .chain(
                                    (schema_dialect == SchemaDialect::Draft201909)
                                        .then_some("$recursiveAnchor"),
                                )
                                .filter_map(|key| {
                                    value.get(key).map(|value| (key.into(), value.clone()))
                                })
                                .collect(),
                        )
                    } else {
                        Value::Null
                    };
                    let has_reference = syntax::reference_keywords(context, schema_dialect)
                        .iter()
                        .any(|key| value.get(key).is_some());
                    nodes.push((
                        pointer.to_string(),
                        context,
                        metadata,
                        has_reference,
                        schema_dialect,
                    ));
                }
            },
        );
        if context == Context::Literal {
            nodes.push((
                start.into(),
                Context::Literal,
                Value::Null,
                false,
                schema_default,
            ));
        }
        if let Some(source) = source {
            self.sources.insert(uri.into(), source);
        }
        let root_base: Arc<str> = uri.into();
        let root_resource = Arc::new(root.clone());
        // Parent contexts precede children in traversal order. Maps and arrays also
        // have entries so resource scope crosses collection boundaries correctly.
        for (path, context, value, has_reference, schema_dialect) in nodes {
            let address = Address {
                uri: uri.into(),
                pointer: path.clone(),
            };
            let parent = path.rsplit_once('/').and_then(|(p, _)| {
                self.contexts.get(&Address {
                    uri: uri.into(),
                    pointer: p.into(),
                })
            });
            let mut base = parent.map_or_else(|| root_base.clone(), |p| p.base.clone());
            let mut resource = parent.map_or_else(|| root_resource.clone(), |p| p.resource.clone());
            if api_dialect.modern_schema() && context == Context::Schema {
                if let Some(id) = value.get(schema_dialect.identifier()) {
                    if let Some(id) = id.as_str() {
                        let url = pointer::checked_url(id, &base, &address)?;
                        if let Some(fragment) =
                            url.fragment().filter(|fragment| !fragment.is_empty())
                        {
                            if !schema_dialect.ignores_reference_siblings()
                                || !id.starts_with('#')
                                || !valid_legacy_anchor(fragment)
                            {
                                return Err(ParserError::new(
                                    "INVALID_SCHEMA_ID",
                                    "Nonempty legacy schema identifier fragments require a plain-name anchor",
                                    address,
                                ));
                            }
                            let key = format!("{}#{fragment}", pointer::document_uri(&url));
                            if self
                                .anchors
                                .insert(key, address.clone())
                                .is_some_and(|previous| previous != address)
                            {
                                return Err(ParserError::new(
                                    "DUPLICATE_ANCHOR",
                                    "Schema identifier has more than one definition",
                                    address,
                                ));
                            }
                        } else {
                            base = pointer::document_uri(&url).into();
                            resource = Arc::new(address.clone());
                            self.insert_resource(base.to_string(), address.clone())?;
                        }
                    } else {
                        return Err(ParserError::new(
                            "INVALID_SCHEMA_ID",
                            "$id must be a string",
                            address,
                        ));
                    }
                }
                if let Some(anchor) = value.get("$recursiveAnchor")
                    && !anchor.is_boolean()
                {
                    return Err(ParserError::new(
                        "INVALID_RECURSIVE_ANCHOR",
                        "$recursiveAnchor must be a boolean",
                        Address {
                            uri: address.uri.clone(),
                            pointer: pointer::child(&path, "$recursiveAnchor"),
                        },
                    ));
                }
                for &key in syntax::anchor_keywords(schema_dialect) {
                    if let Some(anchor) = value.get(key) {
                        if let Some(anchor) = anchor.as_str().filter(|a| valid_anchor(a)) {
                            let key = format!("{base}#{anchor}");
                            if let Some(previous) = self.anchors.insert(key, address.clone())
                                && previous != address
                            {
                                return Err(ParserError::new(
                                    "DUPLICATE_ANCHOR",
                                    "Schema anchor has more than one definition",
                                    address,
                                ));
                            }
                        } else {
                            return Err(ParserError::new(
                                "INVALID_ANCHOR",
                                "Schema anchor must be a valid plain-name fragment",
                                address,
                            ));
                        }
                    }
                }
            }
            if has_reference {
                self.reference_nodes.insert(address.clone());
            }
            self.contexts.insert(
                address,
                NodeContext {
                    context,
                    schema_dialect,
                    api_dialect,
                    base,
                    resource,
                },
            );
        }
        Ok(())
    }

    fn insert_resource(&mut self, uri: String, address: Address) -> Result<()> {
        if let Some(final_uri) = self.retrieval_aliases.get(&uri)
            && (address.uri != *final_uri || !address.pointer.is_empty())
        {
            return Err(ParserError::new(
                "DUPLICATE_RESOURCE",
                "Schema resource conflicts with a retrieval alias",
                address,
            ));
        }
        if let Some(previous) = self.resources.get(&uri)
            && previous != &address
        {
            return Err(ParserError::new(
                "DUPLICATE_RESOURCE",
                format!("Schema resource URI already registered: {uri}"),
                address,
            ));
        }
        self.resources.insert(uri, address);
        Ok(())
    }

    pub fn select_schema_roots(
        &mut self,
        available: &[Address],
        selected: &[Address],
    ) -> Result<()> {
        // Selection tracks physical addresses, just like indexed schema nodes.
        // Otherwise a redirected registry appears absent and all its roots activate.
        let canonical = |address: &Address| -> Result<Address> {
            let url = pointer::checked_url(&address.uri, &self.root_uri, address)?;
            Ok(Address {
                uri: retrieval::canonical(&self.retrieval_aliases, url.as_str()).into(),
                pointer: address.pointer.clone(),
            })
        };
        let available = available
            .iter()
            .map(canonical)
            .collect::<Result<Vec<_>>>()?;
        let selected = selected.iter().map(canonical).collect::<Result<Vec<_>>>()?;
        self.schema_selection = Some(SchemaSelection::new(&available, &selected));
        Ok(())
    }

    pub fn schema_address_active(&self, address: &Address) -> bool {
        address.uri == self.root_uri
            || self
                .schema_selection
                .as_ref()
                .is_none_or(|selection| selection.includes(address))
    }

    pub fn resolve_all(&mut self) -> Result<()> {
        let mut scanned = BTreeSet::new();
        let mut failed = BTreeSet::new();
        loop {
            let mut candidates = Vec::new();
            let mut errors = Vec::new();
            for address in &self.reference_nodes {
                if !self.schema_address_active(address) {
                    continue;
                }
                let Some(context) = self.contexts.get(address) else {
                    continue;
                };
                if scanned.contains(address) {
                    continue;
                }
                scanned.insert(address.clone());
                let Some(value) = self.value(address) else {
                    continue;
                };
                for &kind in syntax::reference_keywords(context.context, context.schema_dialect) {
                    if kind != "$ref" && !self.dialect.modern_schema() {
                        continue;
                    }
                    if let Some(reference) = value.get(kind) {
                        if let Some(reference) = reference.as_str() {
                            if kind == "$recursiveRef" && reference != "#" {
                                errors.push(ParserError::new(
                                    "UNSUPPORTED_RECURSIVE_REFERENCE",
                                    "Draft 2019-09 defines recursive reference semantics only for the exact fragment #",
                                    Address { uri: address.uri.clone(), pointer: pointer::child(&address.pointer, kind) },
                                ));
                                continue;
                            }
                            candidates.push((
                                address.clone(),
                                context.base.clone(),
                                kind.to_string(),
                                reference.to_string(),
                            ));
                        } else {
                            errors.push(ParserError::new(
                                "INVALID_REFERENCE",
                                format!("{kind} must be a string"),
                                Address {
                                    uri: address.uri.clone(),
                                    pointer: pointer::child(&address.pointer, kind),
                                },
                            ));
                        }
                    }
                }
            }
            for error in errors {
                self.push_error(error);
            }
            if candidates.is_empty() {
                break;
            }
            // Load unknown retrieval documents before reporting missing pointers.
            for (address, base, kind, reference) in &candidates {
                let at = Address {
                    uri: address.uri.clone(),
                    pointer: pointer::child(&address.pointer, kind),
                };
                let url = match pointer::checked_url(reference, base, &at) {
                    Ok(url) => url,
                    Err(error) => {
                        self.push_error(error);
                        continue;
                    }
                };
                if let Err(error) = pointer::fragment(&url, &at) {
                    self.push_error(error);
                    continue;
                }
                let uri = pointer::document_uri(&url);
                if self
                    .resources
                    .contains_key(retrieval::canonical(&self.retrieval_aliases, &uri))
                    || failed.contains(&uri)
                {
                    continue;
                }
                if !self.options.resolve.external {
                    continue;
                }
                let foreign_schema = self.schema_mode
                    && self
                        .contexts
                        .get(address)
                        .is_some_and(|node| node.context == Context::Schema);
                if foreign_schema {
                    self.schema_sources.insert(uri.clone());
                    let dialect = self
                        .contexts
                        .get(address)
                        .map(|context| context.schema_dialect)
                        .unwrap_or_default();
                    self.set_schema_default(
                        &Address {
                            uri: uri.clone(),
                            pointer: String::new(),
                        },
                        dialect,
                    )?;
                }
                let read = self
                    .read(&uri)
                    .and_then(|source| {
                        self.parse_source(&source.retrieval_uri, source.content, false)
                    })
                    .and_then(|source| {
                        if foreign_schema {
                            let inherited_api = self
                                .contexts
                                .get(address)
                                .map_or(self.dialect, |context| context.api_dialect);
                            self.schema_api_defaults
                                .entry(source.uri.clone())
                                .or_insert(inherited_api);
                        }
                        let context = if foreign_schema {
                            Some(
                                if source.document.get("asyncapi").is_some()
                                    || source.document.get("openapi").is_some()
                                    || source.document.get("swagger").is_some()
                                    || source
                                        .text
                                        .trim_start_matches('\u{feff}')
                                        .trim_start()
                                        .starts_with("#%RAML")
                                {
                                    Context::Literal
                                } else {
                                    let dialect = self
                                        .contexts
                                        .get(address)
                                        .map(|context| context.schema_dialect)
                                        .unwrap_or_default();
                                    if syntax::schema_document(&source.document, dialect) {
                                        Context::Schema
                                    } else {
                                        Context::Literal
                                    }
                                },
                            )
                        } else {
                            self.contexts
                                .get(address)
                                .map(|c| c.context)
                                .filter(|context| {
                                    *context == Context::Schema
                                        && url.fragment().is_none_or(|f| !f.starts_with('/'))
                                        && source.document.get("openapi").is_none()
                                        && source.document.get("swagger").is_none()
                                })
                        };
                        self.add(source, context)
                    });
                if let Err(error) = read {
                    if error.diagnostics.iter().any(|d| {
                        matches!(
                            d.code.as_str(),
                            "LIMIT_DOCUMENTS" | "LIMIT_TOTAL_BYTES" | "LIMIT_NODES"
                        )
                    }) {
                        return Err(error);
                    }
                    failed.insert(uri);
                    self.push_error(error);
                    self.check_diagnostics()?;
                }
            }
            for (address, base, kind, reference) in candidates {
                let from = self.locate(&Address {
                    uri: address.uri.clone(),
                    pointer: pointer::child(&address.pointer, &kind),
                });
                match self.target(&reference, &base, &address) {
                    Ok(mut target) => {
                        // Recursive fallback starts at the current resource,
                        // never at an invented component-local resource. An
                        // OpenAPI/container root is not a schema fallback.
                        if kind == "$recursiveRef"
                            && target.as_ref().is_some_and(|target| {
                                self.contexts
                                    .get(target)
                                    .is_none_or(|node| node.context != Context::Schema)
                            })
                        {
                            self.push_error(ParserError::new(
                                "RECURSIVE_REFERENCE_TARGET",
                                "Recursive reference fallback must identify a schema resource; an embedded schema may need an authored $id",
                                from.address.clone(),
                            ));
                            target = None;
                        }
                        if let Some(target) = &target
                            && let Some(selection) = &mut self.schema_selection
                        {
                            selection.activate(target);
                        }
                        if self.dialect.modern_schema()
                            && self
                                .contexts
                                .get(&address)
                                .is_some_and(|node| node.context == Context::Schema)
                            && let Some(target) = &target
                            && self
                                .value(target)
                                .is_some_and(|value| value.is_object() || value.is_boolean())
                            && self
                                .contexts
                                .get(target)
                                .is_none_or(|node| node.context != Context::Schema)
                        {
                            let dialect = self
                                .contexts
                                .get(&address)
                                .map(|context| context.schema_dialect)
                                .unwrap_or_default();
                            if !self.schema_defaults.contains_key(target) {
                                self.set_schema_default(target, dialect)?;
                            }
                            self.add_schema_root(target)?;
                        }
                        let resolved = target.is_some();
                        if !resolved {
                            let external = pointer::checked_url(&reference, &base, &address)
                                .is_ok_and(|u| {
                                    !self.resources.contains_key(&pointer::document_uri(&u))
                                });
                            let skipped = external && !self.options.resolve.external;
                            self.diagnostics.push(Diagnostic {
                                code: if skipped {
                                    "EXTERNAL_REFERENCE_SKIPPED"
                                } else {
                                    "UNRESOLVED_REFERENCE"
                                }
                                .into(),
                                severity: if skipped {
                                    Severity::Warning
                                } else {
                                    Severity::Error
                                },
                                message: format!("Cannot resolve {reference}"),
                                location: from.clone(),
                                related: Vec::new(),
                            });
                        }
                        let Some(slot) = syntax::reference_slot(&kind) else {
                            continue;
                        };
                        self.reference_index
                            .entry(address.clone())
                            .or_insert([None; 3])[slot] = Some(self.references.len());
                        self.references.push(ReferenceEdge {
                            from,
                            reference,
                            kind,
                            target: target.map(|t| self.locate(&t)),
                            resolved,
                        });
                    }
                    Err(error) => self.push_error(error),
                }
                self.check_diagnostics()?;
            }
        }
        self.reference_sibling_diagnostics();
        for (kind, code) in [
            ("$dynamicRef", "DYNAMIC_REFERENCE_PRESERVED"),
            ("$recursiveRef", "RECURSIVE_REFERENCE_PRESERVED"),
        ] {
            if let Some(edge) = self.references.iter().find(|edge| edge.kind == kind) {
                self.push_diagnostic(Diagnostic { code: code.into(), severity: Severity::Info,
                    message: "Dynamic-scope references are preserved for a JSON Schema evaluator; the parser indexes their static fallback without evaluating dynamic scope".into(), location: edge.from.clone(), related: Vec::new() });
            }
        }
        Ok(())
    }

    fn reference_sibling_diagnostics(&mut self) {
        let mut warnings = Vec::new();
        for address in &self.reference_nodes {
            if !self.schema_address_active(address) {
                continue;
            }
            if warnings.len() > self.options.limits.max_diagnostics {
                break;
            }
            let Some(context) = self.contexts.get(address) else {
                continue;
            };
            let Some(value) = self.value(address) else {
                continue;
            };
            if value.get("$ref").is_none() {
                continue;
            }
            for key in value.as_object().into_iter().flatten().map(|(key, _)| key) {
                if !syntax::schema_reference_sibling_allowed(
                    context.context,
                    context.api_dialect,
                    context.schema_dialect,
                    &address.pointer,
                    value,
                    key,
                ) {
                    warnings.push(Diagnostic {
                        code: "IGNORED_REFERENCE_SIBLING".into(),
                        severity: Severity::Warning,
                        message: format!(
                            "Reference Object sibling {key} is ignored by this OpenAPI dialect"
                        ),
                        location: self.locate(&Address {
                            uri: address.uri.clone(),
                            pointer: pointer::child(&address.pointer, key),
                        }),
                        related: Vec::new(),
                    });
                }
            }
        }
        for warning in warnings {
            self.push_diagnostic(warning);
        }
    }

    pub fn target(&self, reference: &str, base: &str, at: &Address) -> Result<Option<Address>> {
        let url = pointer::checked_url(reference, base, at)?;
        let fragment = pointer::fragment(&url, at)?;
        let requested_uri = pointer::document_uri(&url);
        let uri = retrieval::canonical(&self.retrieval_aliases, &requested_uri);
        if !fragment.is_empty() && !fragment.starts_with('/') {
            // An authored $id may equal a retrieval alias at the same root.
            // Its anchors retain that logical identity rather than being renamed.
            return Ok(self
                .anchors
                .get(&format!("{requested_uri}#{fragment}"))
                .or_else(|| self.anchors.get(&format!("{uri}#{fragment}")))
                .cloned());
        }
        let Some(resource) = self.resources.get(uri) else {
            return Ok(None);
        };
        let address = Address {
            uri: resource.uri.clone(),
            pointer: format!("{}{}", resource.pointer, fragment),
        };
        Ok(self.value(&address).map(|_| address))
    }

    pub fn edge(&self, address: &Address, kind: &str) -> Option<&ReferenceEdge> {
        let slot = syntax::reference_slot(kind)?;
        let index = self.reference_index.get(address)?[slot]?;
        self.references.get(index)
    }

    pub fn reference_index(&self) -> crate::ReferenceIndex {
        crate::ReferenceIndex {
            root_uri: self.root_uri.clone(),
            root_document: self.working_root.clone(),
            resources: self.resources.clone(),
            anchors: self.anchors.clone(),
            retrieval_aliases: self.retrieval_aliases.clone(),
        }
    }

    pub fn value(&self, address: &Address) -> Option<&Value> {
        if address.uri == self.root_uri
            && let Some(root) = &self.working_root
        {
            return pointer::get(root, &address.pointer);
        }
        pointer::get(&self.sources.get(&address.uri)?.document, &address.pointer)
    }

    pub fn locate(&self, address: &Address) -> Location {
        self.sources.get(&address.uri).map_or_else(
            || address.clone().into(),
            |source| source.locate(&address.pointer),
        )
    }

    pub fn push_diagnostic(&mut self, mut diagnostic: Diagnostic) {
        // A loader failure can carry an exact position before its source was
        // registered. Do not replace that position with an address-only lookup.
        if diagnostic.location.line.is_none()
            && diagnostic.location.column.is_none()
            && diagnostic.location.offset.is_none()
        {
            diagnostic.location = self.locate(&diagnostic.location.address);
        }
        crate::diagnostics::append(
            &mut self.diagnostics,
            self.options.limits.max_diagnostics,
            diagnostic,
        );
    }

    pub fn push_error(&mut self, error: ParserError) {
        for diagnostic in error.diagnostics {
            self.push_diagnostic(diagnostic);
        }
    }

    pub fn check_diagnostics(&self) -> Result<()> {
        if self.diagnostics.len() >= self.options.limits.max_diagnostics {
            return Err(crate::diagnostics::exhausted(Address {
                uri: self.root_uri.clone(),
                pointer: String::new(),
            }));
        }
        Ok(())
    }

    /// Effective API fields with physical source addresses. Path Item siblings
    /// override referenced fields; other Reference Objects override annotations.
    pub fn api_fields(&self, start: &Address, path_item: bool) -> BTreeMap<String, Address> {
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        let mut address = start.clone();
        while seen.len() <= self.options.limits.max_depth && seen.insert(address.clone()) {
            chain.push(address.clone());
            let Some(target) = self
                .edge(&address, "$ref")
                .and_then(|edge| edge.target.as_ref())
            else {
                break;
            };
            address = target.address.clone();
        }
        let mut fields = BTreeMap::new();
        for (index, address) in chain.iter().rev().enumerate() {
            let Some(map) = self.value(address).and_then(Value::as_object) else {
                continue;
            };
            for key in map.keys().filter(|key| key.as_str() != "$ref") {
                if index == 0
                    || path_item
                    || self.dialect.modern_schema()
                        && matches!(key.as_str(), "summary" | "description")
                {
                    fields.insert(
                        key.clone(),
                        Address {
                            uri: address.uri.clone(),
                            pointer: pointer::child(&address.pointer, key),
                        },
                    );
                }
            }
        }
        fields
    }

    pub fn resolved_api_value(&self, address: &Address) -> Value {
        let target = self.follow(address.clone());
        if self
            .value(&target)
            .is_some_and(|value| value.get("$ref").is_some())
        {
            return self.value(&target).cloned().unwrap_or(Value::Null);
        }
        if !self.value(&target).is_some_and(Value::is_object) {
            return self.value(&target).cloned().unwrap_or(Value::Null);
        }
        Value::Object(
            self.api_fields(address, false)
                .into_iter()
                .filter_map(|(key, address)| {
                    self.value(&address).cloned().map(|value| (key, value))
                })
                .collect(),
        )
    }

    /// Follow only reference objects, retaining the true source of the final node.
    pub fn follow(&self, mut address: Address) -> Address {
        let mut seen = BTreeSet::new();
        while seen.insert(address.clone()) {
            let Some(target) = self.edge(&address, "$ref").and_then(|e| e.target.as_ref()) else {
                break;
            };
            address = target.address.clone();
        }
        address
    }
}

fn valid_anchor(anchor: &str) -> bool {
    let mut chars = anchor.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

fn valid_legacy_anchor(anchor: &str) -> bool {
    let mut chars = anchor.chars();
    chars
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic())
        && chars.all(|character| character.is_ascii_alphanumeric() || "-_:.".contains(character))
}
