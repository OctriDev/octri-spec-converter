//! Portable bundling. Copy referenced resources once, then rewrite reference
//! addresses against their output locations. Schema resource boundaries retain
//! absolute IDs; refs leaving a resource use a stable generated resource ID.

use crate::{Address, Dialect, ParserError, Result, SchemaDialect};
use crate::{
    pointer,
    session::Session,
    syntax::{self, Context},
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub(crate) struct Bundle {
    pub document: Value,
    pub origins: BTreeMap<String, Address>,
}

pub(crate) fn bundle(session: &Session<'_>) -> Result<Value> {
    Ok(build(session, false)?.document)
}

pub(crate) fn bundle_with_sources(session: &Session<'_>) -> Result<Bundle> {
    build(session, true)
}

fn build(session: &Session<'_>, capture_locations: bool) -> Result<Bundle> {
    let mut builder = Builder {
        session,
        capture_locations,
        mapping: BTreeMap::new(),
        reference_addresses: session
            .references
            .iter()
            .flat_map(|edge| {
                std::iter::once(parent_address(&edge.from.address))
                    .chain(edge.target.as_ref().map(|target| target.address.clone()))
            })
            .collect(),
        origins: BTreeMap::new(),
        generated_containers: BTreeSet::new(),
        nodes: 0,
        copied_bytes: 0,
    };
    let root = Address {
        uri: session.root_uri.clone(),
        pointer: String::new(),
    };
    let original = session.value(&root).ok_or_else(|| {
        ParserError::new("INVALID_DOCUMENT", "Root document is missing", root.clone())
    })?;
    builder.register_tree(&root, "", original);
    // Whole standalone schema documents are embedded, not individual fragments.
    // This keeps sibling $defs, anchors and dynamic scope in the same resource.
    let mut imports = Vec::new();
    let schema_map = if session.dialect == Dialect::Swagger2 {
        "/definitions"
    } else {
        "/components/schemas"
    };
    let mut used_names: BTreeSet<String> = original
        .pointer(schema_map)
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    for (uri, source) in &session.sources {
        if uri == &session.root_uri {
            continue;
        }
        let address = Address {
            uri: uri.clone(),
            pointer: String::new(),
        };
        let whole_schema = session
            .contexts
            .get(&address)
            .is_some_and(|node| node.context == Context::Schema);
        if whole_schema && !session.schema_address_active(&address) {
            continue;
        }
        // A Draft 7 reference cannot also establish a resource with `$id`:
        // all of its siblings are ignored. Give it a containing resource.
        let reference_resource = whole_schema
            && session
                .contexts
                .get(&address)
                .is_some_and(|node| node.schema_dialect.ignores_reference_siblings())
            && source.document.get("$ref").is_some();
        let roots = if reference_resource {
            vec![address.clone()]
        } else if whole_schema {
            Vec::new()
        } else if session.schema_sources.contains(uri) {
            outer_schema_roots(session, uri)
        } else {
            Vec::new()
        };
        if !whole_schema && roots.is_empty() {
            continue;
        }
        let mut name = format!("OctriExternal{}", imports.len() + 1);
        while used_names.contains(&name) {
            name.push('_');
        }
        used_names.insert(name.clone());
        let path = pointer::child(schema_map, &name);
        let whole_schema = whole_schema && !reference_resource;
        let schema_dialect = roots
            .iter()
            .find_map(|root| {
                session
                    .contexts
                    .get(root)
                    .map(|node| node.schema_dialect)
                    .filter(|dialect| dialect.ignores_reference_siblings())
            })
            .unwrap_or_default();
        let definitions_key = if schema_dialect.ignores_reference_siblings() {
            "definitions"
        } else {
            "$defs"
        };
        if whole_schema {
            builder.register_tree(&address, &path, &source.document);
        } else {
            // A foreign document can contain several schemas in one resource.
            // Mount them under one $defs registry, retaining their common base
            // and dynamic scope without activating unrelated foreign fields.
            // Generated containers also retain a physical origin. Consumers
            // need it to migrate the container's dialect and identity together
            // with its selected schemas, including reference-only resources.
            builder.origins.insert(path.clone(), address.clone());
            builder.generated_containers.insert(path.clone());
            for (index, root) in roots.iter().enumerate() {
                let value = session.value(root).ok_or_else(|| {
                    ParserError::new(
                        "INVALID_SCHEMA_ROOT",
                        "Schema root is missing",
                        root.clone(),
                    )
                })?;
                builder.register_tree(
                    root,
                    &format!("{path}/{definitions_key}/Schema{}", index + 1),
                    value,
                );
            }
        }
        imports.push((name, path, address, roots, schema_dialect, definitions_key));
    }
    let mut output = builder.copy(&root, "", Context::Api, 0)?;
    for (name, path, address, roots, schema_dialect, definitions_key) in imports {
        let value = if roots.is_empty() {
            let mut value = builder.copy(&address, &path, Context::Schema, 0)?;
            if session.dialect.modern_schema()
                && session
                    .contexts
                    .get(&address)
                    .is_some_and(|node| node.schema_dialect.ignores_reference_siblings())
                && let Some(map) = value.as_object_mut()
            {
                map.entry("$schema").or_insert_with(|| {
                    Value::String(
                        session
                            .contexts
                            .get(&address)
                            .map(|node| node.schema_dialect)
                            .unwrap_or_default()
                            .uri()
                            .into(),
                    )
                });
            }
            value
        } else {
            let mut definitions = Map::new();
            for (index, root) in roots.iter().enumerate() {
                let name = format!("Schema{}", index + 1);
                definitions.insert(
                    name.clone(),
                    builder.copy_schema(
                        root,
                        &pointer::child(&format!("{path}/{definitions_key}"), &name),
                        schema_dialect,
                        2,
                    )?,
                );
            }
            let mut value = serde_json::json!({schema_dialect.identifier(): address.uri, definitions_key: definitions});
            if schema_dialect.ignores_reference_siblings() {
                value["$schema"] = Value::String(schema_dialect.uri().into());
            }
            value
        };
        insert_schema(&mut output, session.dialect, name, value)?;
    }
    builder.rewrite(&mut output)?;
    Ok(Bundle {
        document: output,
        origins: builder.origins,
    })
}

fn outer_schema_roots(session: &Session<'_>, uri: &str) -> Vec<Address> {
    session
        .contexts
        .range(
            Address {
                uri: uri.into(),
                pointer: String::new(),
            }..,
        )
        .take_while(|(address, _)| address.uri == uri)
        .filter(|(address, node)| {
            node.context == Context::Schema && session.schema_address_active(address) && {
                let mut parent = address.pointer.as_str();
                let mut nested = false;
                while let Some((prefix, _)) = parent.rsplit_once('/') {
                    let ancestor = Address {
                        uri: uri.into(),
                        pointer: prefix.into(),
                    };
                    if session
                        .contexts
                        .get(&ancestor)
                        .is_some_and(|node| node.context == Context::Schema)
                    {
                        // Ignored siblings are not copied with a legacy
                        // reference. An explicitly reached schema below one
                        // therefore needs its own emitted root.
                        nested = !legacy_reference(session, &ancestor);
                        break;
                    }
                    parent = prefix;
                }
                !nested
            }
        })
        .map(|(address, _)| address.clone())
        .collect()
}

fn context_identifier(session: &Session<'_>, address: &Address) -> &'static str {
    session
        .contexts
        .get(address)
        .map(|node| node.schema_dialect)
        .unwrap_or_default()
        .identifier()
}

fn value_identifier(map: &Map<String, Value>) -> &'static str {
    map.get("$schema")
        .and_then(Value::as_str)
        .and_then(SchemaDialect::from_uri)
        .unwrap_or_default()
        .identifier()
}

fn legacy_reference(session: &Session<'_>, address: &Address) -> bool {
    session.contexts.get(address).is_some_and(|context| {
        context.context == Context::Schema && context.schema_dialect.ignores_reference_siblings()
    }) && session
        .value(address)
        .is_some_and(|value| value.get("$ref").is_some())
}

struct Builder<'a, 'b> {
    session: &'a Session<'b>,
    // Strict validation needs every output origin for error locations. Bundling
    // needs only schema origins and reference addresses for resource rewriting.
    capture_locations: bool,
    mapping: BTreeMap<Address, String>,
    // Only ref holders and targets need reverse output-address lookup.
    reference_addresses: HashSet<Address>,
    origins: BTreeMap<String, Address>,
    // These nodes carry source provenance, but are not copies of source refs.
    generated_containers: BTreeSet<String>,
    nodes: usize,
    copied_bytes: usize,
}

impl Builder<'_, '_> {
    fn schema_reference_with_siblings(
        &mut self,
        address: &Address,
        path: &str,
        value: &Value,
        target: &Address,
        depth: usize,
    ) -> Result<Value> {
        let mut name = "OctriInline".to_owned();
        while value
            .get("$defs")
            .is_some_and(|definitions| definitions.get(&name).is_some())
        {
            name.push('_');
        }
        let imported_path = pointer::child(&pointer::child(path, "$defs"), &name);
        let target_value = self.session.value(target).ok_or_else(|| {
            ParserError::new(
                "UNRESOLVED_REFERENCE",
                "Bundle target is missing",
                target.clone(),
            )
        })?;
        self.register_tree(target, &imported_path, target_value);
        self.origins.insert(imported_path.clone(), target.clone());
        let dialect = self
            .session
            .contexts
            .get(address)
            .map(|context| context.schema_dialect)
            .unwrap_or_default();
        let imported = self.copy_schema(target, &imported_path, dialect, depth + 2)?;
        // The target now has a location, so copying the original reference no
        // longer inlines it. Every conjunct and its resource identity survives.
        let mut output = self.copy_value(address, path, value, Context::Schema, depth + 1)?;
        let definitions = output
            .as_object_mut()
            .ok_or_else(|| {
                ParserError::new(
                    "INVALID_SCHEMA",
                    "Schema reference with siblings must be an object",
                    address.clone(),
                )
            })?
            .entry("$defs")
            .or_insert_with(|| Value::Object(Map::new()));
        definitions
            .as_object_mut()
            .ok_or_else(|| {
                ParserError::new("INVALID_SCHEMA", "$defs must be an object", address.clone())
            })?
            .insert(name, imported);
        Ok(output)
    }

    fn inherited_dialect(&self, path: &str) -> SchemaDialect {
        let mut parent = path;
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            if let Some(context) = self
                .origins
                .get(prefix)
                .and_then(|origin| self.session.contexts.get(origin))
                .filter(|context| context.context == Context::Schema)
            {
                return context.schema_dialect;
            }
            parent = prefix;
        }
        self.session
            .value(&Address {
                uri: self.session.root_uri.clone(),
                pointer: String::new(),
            })
            .and_then(|root| root.get("jsonSchemaDialect"))
            .and_then(Value::as_str)
            .and_then(SchemaDialect::from_uri)
            .unwrap_or_default()
    }

    fn copied_from(&self, root: &Address, descendant: &Address) -> bool {
        let mut pointer = descendant.pointer.as_str();
        while pointer != root.pointer {
            let Some((parent, key)) = pointer.rsplit_once('/') else {
                break;
            };
            if !matches!(key, "$ref" | "$schema")
                && legacy_reference(
                    self.session,
                    &Address {
                        uri: root.uri.clone(),
                        pointer: parent.into(),
                    },
                )
            {
                return false;
            }
            pointer = parent;
        }
        true
    }
    fn copy_schema(
        &mut self,
        root: &Address,
        path: &str,
        parent: SchemaDialect,
        depth: usize,
    ) -> Result<Value> {
        let mut value = self.copy(root, path, Context::Schema, depth)?;
        if let Some(uri) = self.session.schema_uri(root)
            && SchemaDialect::from_uri(uri).is_none()
            && let Some(map) = value.as_object_mut()
        {
            // Preserve an owning OpenAPI document's custom default. Its URI is
            // evidence of an unverified vocabulary, not a modern-dialect hint.
            map.entry("$schema")
                .or_insert_with(|| Value::String(uri.into()));
            map.entry(context_identifier(self.session, root))
                .or_insert_with(|| Value::String(stable_id(root)));
        }
        if let Some(context) = self.session.contexts.get(root)
            && context.schema_dialect != parent
            && let Some(map) = value.as_object_mut()
        {
            map.entry("$schema")
                .or_insert_with(|| Value::String(context.schema_dialect.uri().into()));
            // Mixed dialects get an explicit resource boundary. References to
            // this node are rewritten against the same generated identity.
            map.entry(context_identifier(self.session, root))
                .or_insert_with(|| Value::String(stable_id(root)));
        }
        Ok(value)
    }
    fn register_tree(&mut self, address: &Address, path: &str, value: &Value) {
        if !self.capture_locations {
            self.register_indexed(address, path);
            return;
        }
        if self.reference_addresses.contains(address) {
            self.mapping
                .entry(address.clone())
                .or_insert_with(|| path.into());
        }
        self.origins
            .entry(path.into())
            .or_insert_with(|| address.clone());
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    if !matches!(key.as_str(), "$ref" | "$schema")
                        && legacy_reference(self.session, address)
                    {
                        continue;
                    }
                    self.register_tree(
                        &Address {
                            uri: address.uri.clone(),
                            pointer: pointer::child(&address.pointer, key),
                        },
                        &pointer::child(path, key),
                        value,
                    );
                }
            }
            Value::Array(values) => {
                for (i, value) in values.iter().enumerate() {
                    self.register_tree(
                        &Address {
                            uri: address.uri.clone(),
                            pointer: pointer::child(&address.pointer, &i.to_string()),
                        },
                        &pointer::child(path, &i.to_string()),
                        value,
                    );
                }
            }
            _ => {}
        }
    }

    fn register_indexed(&mut self, root: &Address, path: &str) {
        // The session already indexed schema nodes and resolved ref addresses.
        // Rebase those addresses instead of walking and allocating paths for
        // every scalar and literal payload. Strict diagnostics use the full
        // traversal above; copy_value still enforces every output budget.
        for address in &self.reference_addresses {
            if address.uri == root.uri
                && contains(&root.pointer, &address.pointer)
                && self.copied_from(root, address)
            {
                let output = format!("{path}{}", &address.pointer[root.pointer.len()..]);
                self.mapping
                    .entry(address.clone())
                    .or_insert_with(|| output.clone());
                self.origins
                    .entry(output)
                    .or_insert_with(|| address.clone());
            }
        }
        for (address, context) in
            self.session
                .contexts
                .range(root.clone()..)
                .take_while(|(address, _)| {
                    address.uri == root.uri && address.pointer.starts_with(&root.pointer)
                })
        {
            if context.context == Context::Schema
                && contains(&root.pointer, &address.pointer)
                && self.copied_from(root, address)
            {
                let output = format!("{path}{}", &address.pointer[root.pointer.len()..]);
                self.origins
                    .entry(output)
                    .or_insert_with(|| address.clone());
            }
        }
    }

    fn copy(
        &mut self,
        address: &Address,
        path: &str,
        context: Context,
        depth: usize,
    ) -> Result<Value> {
        let value = self.session.value(address).ok_or_else(|| {
            ParserError::new(
                "UNRESOLVED_REFERENCE",
                "Bundle target is missing",
                address.clone(),
            )
        })?;
        self.copy_value(address, path, value, context, depth)
    }

    fn copy_value(
        &mut self,
        address: &Address,
        path: &str,
        value: &Value,
        context: Context,
        depth: usize,
    ) -> Result<Value> {
        let context = self
            .session
            .contexts
            .get(address)
            .map_or(context, |node| node.context);
        let api_dialect = self
            .session
            .contexts
            .get(address)
            .map_or(self.session.dialect, |node| node.api_dialect);
        self.nodes += 1;
        if self.nodes > self.session.options.limits.max_nodes
            || depth > self.session.options.limits.max_depth
        {
            return Err(ParserError::new(
                "LIMIT_OUTPUT_NODES",
                "Bundled output exceeds limits",
                address.clone(),
            ));
        }
        let bytes = match value {
            Value::String(value) => value.len(),
            Value::Object(map) => map.keys().map(String::len).sum(),
            _ => 0,
        };
        self.copied_bytes = self.copied_bytes.saturating_add(bytes);
        if self.copied_bytes > self.session.options.limits.max_output_bytes {
            return Err(ParserError::new(
                "LIMIT_OUTPUT_BYTES",
                "Bundle copy exceeds byte budget",
                address.clone(),
            ));
        }
        if matches!(context, Context::Api | Context::Schema)
            && let Some(target) = self
                .session
                .edge(address, "$ref")
                .and_then(|e| e.target.as_ref())
                .map(|l| &l.address)
            && !self.mapping.contains_key(target)
        {
            let dialect = self
                .session
                .contexts
                .get(address)
                .map(|node| node.schema_dialect)
                .unwrap_or_default();
            if context == Context::Schema
                && !dialect.ignores_reference_siblings()
                && value.as_object().is_some_and(|map| map.len() > 1)
            {
                return self.schema_reference_with_siblings(address, path, value, target, depth);
            }
            // API fragments (responses, path items, parameters) are placed
            // at their first use. Register before copying to close cycles.
            let target_value = self.session.value(target).ok_or_else(|| {
                ParserError::new(
                    "UNRESOLVED_REFERENCE",
                    "Bundle target is missing",
                    target.clone(),
                )
            })?;
            let target_context = self
                .session
                .contexts
                .get(target)
                .map_or(context, |c| c.context);
            self.register_tree(target, path, target_value);
            self.origins.insert(path.into(), target.clone());
            let mut imported =
                if target_context == Context::Schema && self.session.dialect.modern_schema() {
                    self.copy_schema(target, path, self.inherited_dialect(path), depth + 1)?
                } else {
                    self.copy(target, path, target_context, depth + 1)?
                };
            if let (Some(own), Some(imported)) = (value.as_object(), imported.as_object_mut()) {
                let path_item = syntax::is_path_item(&address.pointer, value);
                for (key, child_value) in own {
                    if key == "$ref"
                        || !syntax::schema_reference_sibling_allowed(
                            context,
                            api_dialect,
                            dialect,
                            &address.pointer,
                            value,
                            key,
                        )
                        || !(path_item
                            || api_dialect.modern_schema()
                                && matches!(key.as_str(), "summary" | "description"))
                    {
                        continue;
                    }
                    let child = Address {
                        uri: address.uri.clone(),
                        pointer: pointer::child(&address.pointer, key),
                    };
                    imported.insert(
                        key.clone(),
                        self.copy_value(
                            &child,
                            &pointer::child(path, key),
                            child_value,
                            syntax::next_in_dialect(
                                context,
                                key,
                                child_value,
                                self.session.dialect,
                            ),
                            depth + 1,
                        )?,
                    );
                }
            }
            return Ok(imported);
        }
        match value {
            Value::Object(map) => {
                let mut output = Map::new();
                for (key, child_value) in map {
                    if value.get("$ref").is_some()
                        && matches!(context, Context::Api | Context::Schema)
                        && !syntax::schema_reference_sibling_allowed(
                            context,
                            api_dialect,
                            self.session
                                .contexts
                                .get(address)
                                .map(|node| node.schema_dialect)
                                .unwrap_or_default(),
                            &address.pointer,
                            value,
                            key,
                        )
                    {
                        continue;
                    }
                    let value = child_value;
                    let child = Address {
                        uri: address.uri.clone(),
                        pointer: pointer::child(&address.pointer, key),
                    };
                    output.insert(
                        key.clone(),
                        self.copy_value(
                            &child,
                            &pointer::child(path, key),
                            value,
                            syntax::next_in_dialect(context, key, value, self.session.dialect),
                            depth + 1,
                        )?,
                    );
                }
                Ok(Value::Object(output))
            }
            Value::Array(values) => {
                let mut output = Vec::new();
                for (i, child_value) in values.iter().enumerate() {
                    let child = Address {
                        uri: address.uri.clone(),
                        pointer: pointer::child(&address.pointer, &i.to_string()),
                    };
                    output.push(self.copy_value(
                        &child,
                        &pointer::child(path, &i.to_string()),
                        child_value,
                        if context == Context::SchemaArray {
                            Context::Schema
                        } else {
                            context
                        },
                        depth + 1,
                    )?);
                }
                Ok(Value::Array(output))
            }
            _ => Ok(value.clone()),
        }
    }

    fn rewrite(&mut self, output: &mut Value) -> Result<()> {
        let mut resources: BTreeMap<String, String> = BTreeMap::new();
        let mut nodes = Vec::new();
        syntax::visit_schema(
            output,
            "",
            Context::Api,
            self.session.dialect,
            output
                .get("jsonSchemaDialect")
                .and_then(Value::as_str)
                .and_then(SchemaDialect::from_uri)
                .unwrap_or_default(),
            &mut |value, path, context, dialect| {
                if context == Context::Schema {
                    nodes.push((
                        path.to_string(),
                        value
                            .get(dialect.identifier())
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    ));
                }
            },
        );
        // Existing IDs become absolute before the retrieval URI can change.
        for (path, id) in &nodes {
            if id.is_some()
                && let Some(origin) = self.origins.get(path)
                && let Some(context) = self.session.contexts.get(origin)
            {
                let id = id.as_deref().unwrap_or("");
                if context.schema_dialect.ignores_reference_siblings() && id.starts_with('#') {
                    continue;
                }
                // Generated mixed-dialect boundaries are already absolute;
                // authored relative IDs were resolved during source indexing.
                let id = url::Url::parse(id)
                    .map_or_else(|_| context.base.to_string(), |uri| uri.to_string());
                resources.insert(path.clone(), id);
            } else if let Some(id) = id.as_deref()
                && let Ok(uri) = url::Url::parse(id)
            {
                resources.insert(path.clone(), uri.to_string());
            }
        }
        // Imported schema documents without IDs also need a resource identity.
        for (path, origin) in &self.origins {
            if origin.pointer.is_empty()
                && origin.uri != self.session.root_uri
                && self.session.contexts.get(origin).is_some_and(|c| {
                    c.context == Context::Schema
                        && !(c.schema_dialect.ignores_reference_siblings()
                            && self
                                .session
                                .value(origin)
                                .is_some_and(|value| value.get("$ref").is_some()))
                })
                && self.session.dialect.modern_schema()
            {
                resources
                    .entry(path.clone())
                    .or_insert_with(|| stable_id(origin));
            }
        }
        // A ref escaping an embedded schema resource cannot use a document-root
        // fragment. Promote the destination's outer Schema Object when needed.
        for edge in &self.session.references {
            let Some(target) = edge.target.as_ref() else {
                continue;
            };
            let Some(from_path) = self.mapping.get(&parent_address(&edge.from.address)) else {
                continue;
            };
            let Some(target_path) = self.mapping.get(&target.address) else {
                continue;
            };
            let from_resource = nearest(&resources, from_path);
            if from_resource.is_some()
                && nearest(&resources, target_path).is_none()
                && self.session.dialect.modern_schema()
            {
                let ancestor = nodes
                    .iter()
                    .filter(|(p, _)| contains(p, target_path))
                    .min_by_key(|(p, _)| p.len());
                if let Some((path, _)) = ancestor {
                    let origin = self
                        .origins
                        .get(path)
                        .cloned()
                        .unwrap_or_else(|| target.address.clone());
                    resources.insert(path.clone(), stable_id(&origin));
                } else {
                    return Err(ParserError::new(
                        "BUNDLE_RESOURCE_UNSUPPORTED",
                        "A schema reference cannot escape to a non-schema API object",
                        edge.from.address.clone(),
                    ));
                }
            }
        }
        for (path, id) in &resources {
            let value = output.pointer_mut(path).ok_or_else(|| {
                ParserError::new(
                    "INVALID_BUNDLE",
                    "Output resource location is missing",
                    Address {
                        uri: self.session.root_uri.clone(),
                        pointer: path.clone(),
                    },
                )
            })?;
            if value.is_boolean() {
                *value = serde_json::json!({"allOf": [value.clone()]});
            }
            if let Some(map) = value.as_object_mut() {
                let identifier = self
                    .origins
                    .get(path)
                    .and_then(|origin| self.session.contexts.get(origin))
                    .map(|context| context.schema_dialect.identifier())
                    .unwrap_or_else(|| value_identifier(map));
                map.insert(identifier.into(), Value::String(id.clone()));
                if self
                    .origins
                    .get(path)
                    .and_then(|origin| self.session.contexts.get(origin))
                    .is_some_and(|context| context.schema_dialect == SchemaDialect::Draft201909)
                {
                    // A resource can be retrieved independently of its parent.
                    // Carry the inherited dialect when it gains an absolute ID.
                    map.entry("$schema")
                        .or_insert_with(|| Value::String(SchemaDialect::Draft201909.uri().into()));
                }
            }
        }
        for (path, origin) in &self.origins {
            if self.generated_containers.contains(path) {
                continue;
            }
            let Some(context) = self.session.contexts.get(origin) else {
                continue;
            };
            if !matches!(context.context, Context::Api | Context::Schema) {
                continue;
            }
            for &kind in syntax::reference_keywords(context.context, context.schema_dialect) {
                let Some(edge) = self.session.edge(origin, kind) else {
                    continue;
                };
                let Some(target) = edge.target.as_ref() else {
                    continue;
                };
                let Some(target_path) = self.mapping.get(&target.address) else {
                    continue;
                };
                // Replacing # with an absolute URI would turn a recursive
                // reference into undefined 2019-09 behavior. Resource IDs and
                // dialect boundaries carry its original fallback scope.
                if kind == "$recursiveRef" {
                    continue;
                }
                if kind == "$dynamicRef"
                    && target.address.uri != self.session.root_uri
                    && !self.session.schema_sources.contains(&target.address.uri)
                    && self
                        .session
                        .sources
                        .get(&target.address.uri)
                        .is_some_and(|s| {
                            self.session
                                .contexts
                                .get(&Address {
                                    uri: s.uri.clone(),
                                    pointer: String::new(),
                                })
                                .is_none_or(|c| c.context != Context::Schema)
                        })
                {
                    return Err(ParserError::new(
                        "DYNAMIC_RESOURCE_UNSUPPORTED",
                        "External dynamic references require a standalone JSON Schema resource",
                        edge.from.address.clone(),
                    ));
                }
                let reference =
                    if self.session.dialect.modern_schema() && context.context == Context::Schema {
                        if let Some((resource_path, id)) = nearest(&resources, target_path) {
                            if kind == "$dynamicRef" {
                                let url = pointer::checked_url(
                                    &edge.reference,
                                    &context.base,
                                    &edge.from.address,
                                )?;
                                let fragment = pointer::fragment(&url, &edge.from.address)?;
                                if !fragment.is_empty() && !fragment.starts_with('/') {
                                    format!("{id}#{fragment}")
                                } else {
                                    format!(
                                        "{id}{}",
                                        pointer::reference_to(&target_path[resource_path.len()..])
                                    )
                                }
                            } else if context.schema_dialect == SchemaDialect::Draft201909
                                && target_path == resource_path
                            {
                                // The pinned evaluator loses recursive scope
                                // when a resource-root reference ends in an
                                // empty fragment. The bare ID is equivalent
                                // and retains scope in independent evaluators.
                                id.to_string()
                            } else {
                                format!(
                                    "{id}{}",
                                    pointer::reference_to(&target_path[resource_path.len()..])
                                )
                            }
                        } else {
                            pointer::reference_to(target_path)
                        }
                    } else {
                        pointer::reference_to(target_path)
                    };
                if let Some(map) = output.pointer_mut(path).and_then(Value::as_object_mut) {
                    map.insert(kind.into(), Value::String(reference));
                }
            }
        }
        Ok(())
    }
}

fn insert_schema(output: &mut Value, dialect: Dialect, name: String, value: Value) -> Result<()> {
    let root = output.as_object_mut().ok_or_else(|| {
        ParserError::new(
            "INVALID_DOCUMENT",
            "Expected an API object",
            Address::default(),
        )
    })?;
    let map = if dialect == Dialect::Swagger2 {
        root.entry("definitions")
            .or_insert_with(|| Value::Object(Map::new()))
    } else {
        root.entry("components")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| {
                ParserError::new(
                    "INVALID_DOCUMENT",
                    "components must be an object",
                    Address::default(),
                )
            })?
            .entry("schemas")
            .or_insert_with(|| Value::Object(Map::new()))
    };
    map.as_object_mut()
        .ok_or_else(|| {
            ParserError::new(
                "INVALID_DOCUMENT",
                "Schema collection must be an object",
                Address::default(),
            )
        })?
        .insert(name, value);
    Ok(())
}

fn contains(parent: &str, child: &str) -> bool {
    parent == child
        || child
            .strip_prefix(parent)
            .is_some_and(|s| s.starts_with('/'))
}
fn nearest<'a>(
    resources: &'a BTreeMap<String, String>,
    path: &str,
) -> Option<(&'a String, &'a String)> {
    resources
        .iter()
        .filter(|(p, _)| contains(p, path))
        .max_by_key(|(p, _)| p.len())
}
fn parent_address(address: &Address) -> Address {
    Address {
        uri: address.uri.clone(),
        pointer: address
            .pointer
            .rsplit_once('/')
            .map_or("", |(p, _)| p)
            .into(),
    }
}
fn stable_id(address: &Address) -> String {
    // FNV-1a is only a deterministic name, never a security or integrity hash.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in address
        .uri
        .bytes()
        .chain([b'#'])
        .chain(address.pointer.bytes())
    {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    format!("urn:octri:bundle:{hash:016x}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::{NoIo, Options, ValidationMode};

    #[test]
    fn sparse_origins_preserve_bundle_values_and_errors() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("../../../fixtures/cases.json")).unwrap();
        let validation_cases: Vec<Value> =
            serde_json::from_str(include_str!("../../../fixtures/validation-cases.json")).unwrap();
        let external = serde_json::json!({
            "name": "external API subtree with overlapping pointer prefixes",
            "input": {
                "openapi": "3.1.2", "info": {"title": "Origins", "version": "1"},
                "paths": {"/pets": {"$ref": "api.json#/components/pathItems/A"}}
            },
            "options": {"files": {"api.json": serde_json::json!({
                "openapi": "3.1.2", "info": {"title": "External", "version": "1"},
                "paths": {}, "components": {
                    "pathItems": {
                        "A": {"get": {"responses": {"200": {
                            "description": "OK", "content": {"application/json": {
                                "schema": {"$ref": "#/components/schemas/Result"}
                            }}
                        }}}},
                        "A-": {"description": "Sibling outside imported subtree"}
                    },
                    "schemas": {"Result": {"type": "object", "properties": {
                        "雪/~": {"type": "string"}
                    }}}
                }
            }).to_string()}}
        });
        let mut compared = 0;
        let mut external_compared = 0;
        for fixture in cases.into_iter().chain(validation_cases).chain([external]) {
            for mode in [ValidationMode::Strict, ValidationMode::Compatible] {
                let mut options: Options = serde_json::from_value(
                    fixture
                        .get("options")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                )
                .unwrap();
                options.validate.mode = mode;
                let mut resolver = NoIo;
                let mut session = Session::new(
                    "memory://octri/openapi.yaml".into(),
                    &options,
                    &mut resolver,
                    true,
                )
                .unwrap();
                let content = fixture["input"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| fixture["input"].to_string());
                if session.initialize(Some(content), false, false).is_err()
                    || session.resolve_all().is_err()
                {
                    continue;
                }
                let full = bundle_with_sources(&session).map(|result| result.document);
                if fixture["name"] == "external API subtree with overlapping pointer prefixes" {
                    assert!(full.is_ok());
                    external_compared += 1;
                }
                assert_eq!(
                    serde_json::to_value(bundle(&session)).unwrap(),
                    serde_json::to_value(full).unwrap(),
                    "{} / {mode:?}",
                    fixture["name"],
                );
                compared += 1;
            }
        }
        assert!(compared > 20);
        assert_eq!(external_compared, 2);
    }
}
