//! Immutable, explicitly supplied source files. No URI grants an I/O capability.

use crate::{ConversionError, ConversionRequest, Limits, Result, SourceFile};
use octri_parser::{Address, Location, ReferenceEdge, SourceDocument, SyntaxDocument};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use url::Url;

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn child(pointer: &str, key: &str) -> String {
    format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"))
}

/// Decode a document-local URI fragment before applying JSON Pointer escapes.
/// This layer accepts no external resources and never performs I/O.
pub(crate) fn local_pointer(reference: &str) -> Result<String> {
    let fragment = reference.strip_prefix('#').ok_or_else(|| {
        ConversionError::new(
            "UNRESOLVED_REFERENCE",
            "Bundle external references before converting transport objects",
        )
    })?;
    let pointer = percent_encoding::percent_decode_str(fragment)
        .decode_utf8()
        .map_err(|_| {
            ConversionError::new(
                "REFERENCE_FRAGMENT",
                "Reference fragment must be valid UTF-8",
            )
        })?
        .into_owned();
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(ConversionError::new(
            "REFERENCE_FRAGMENT",
            "Transport object references require JSON Pointers",
        ));
    }
    Ok(pointer)
}

#[derive(Debug, Clone)]
pub(crate) enum SchemaFragment {
    Json(Address),
    Xml { uri: String, name: Option<String> },
}

#[derive(Debug)]
pub(crate) struct Sources {
    pub root: String,
    pub files: BTreeMap<String, String>,
    pub documents: BTreeMap<String, SourceDocument>,
    pub includes: BTreeMap<Address, String>,
    pub origins: BTreeMap<String, Address>,
    pub schema_fragments: BTreeMap<Address, SchemaFragment>,
    pub schema_references: Vec<ReferenceEdge>,
    pub limits: Limits,
    text_locations: BTreeMap<Address, Location>,
    parsed_nodes: usize,
    derived_resources: std::collections::BTreeSet<String>,
    derived_bytes: usize,
    derived_nodes: usize,
}

impl Sources {
    pub fn new(request: &ConversionRequest) -> Result<Self> {
        request.config.limits.validate()?;
        let root = canonical_uri(&request.input.uri)?;
        let content = request.input.content.clone().ok_or_else(|| {
            ConversionError::new(
                "INPUT_REQUIRED",
                "Supply source text; conversion does not read a path or URL",
            )
        })?;
        let mut files = BTreeMap::from([(root.clone(), content)]);
        let base = Url::parse(&root)
            .map_err(|error| ConversionError::new("INVALID_URI", error.to_string()))?;
        for (name, text) in &request.files {
            let uri = base
                .join(name)
                .map_err(|error| ConversionError::new("INVALID_URI", error.to_string()))?;
            let uri = canonical_uri(uri.as_str())?;
            if let Some(previous) = files.get(&uri) {
                if previous != text {
                    return Err(ConversionError::new(
                        "CONFLICTING_SOURCE",
                        format!("Multiple contents for {uri}"),
                    ));
                }
            } else {
                files.insert(uri, text.clone());
            }
        }
        let limits = &request.config.limits;
        if files.len() > limits.max_documents
            || files
                .values()
                .any(|text| text.len() > limits.max_document_bytes)
            || files
                .values()
                .try_fold(0usize, |total, text| total.checked_add(text.len()))
                .is_none_or(|total| total > limits.max_total_bytes)
        {
            return Err(ConversionError::new(
                "LIMIT_INPUT",
                "Source bundle exceeds published file or byte limits",
            ));
        }
        Ok(Self {
            root,
            files,
            documents: BTreeMap::new(),
            includes: BTreeMap::new(),
            origins: BTreeMap::new(),
            schema_fragments: BTreeMap::new(),
            schema_references: Vec::new(),
            limits: limits.clone(),
            text_locations: BTreeMap::new(),
            parsed_nodes: 0,
            derived_resources: Default::default(),
            derived_bytes: 0,
            derived_nodes: 0,
        })
    }

    pub fn text(&self, uri: &str) -> Result<&str> {
        self.files.get(uri).map(String::as_str).ok_or_else(|| {
            ConversionError::new(
                "SOURCE_NOT_SUPPLIED",
                format!("Supply the referenced source file explicitly: {uri}"),
            )
        })
    }

    /// Derived JSON and XML share one admission pool. Originals retain their
    /// separate immutable input budget; repeated imports charge only once.
    pub fn admit_derived(&mut self, identity: &str, bytes: usize, nodes: usize) -> Result<()> {
        if self.derived_resources.contains(identity) {
            return Ok(());
        }
        if self.derived_resources.len() >= self.limits.max_documents
            || self.derived_bytes.saturating_add(bytes) > self.limits.max_total_bytes
            || self.derived_nodes.saturating_add(nodes) > self.limits.max_nodes
        {
            return Err(derived_limit());
        }
        self.derived_resources.insert(identity.into());
        self.derived_bytes += bytes;
        self.derived_nodes += nodes;
        Ok(())
    }

    pub fn charge_derived_nodes(&mut self, nodes: usize) -> Result<()> {
        let total = self.derived_nodes.saturating_add(nodes);
        if total > self.limits.max_nodes {
            return Err(derived_limit());
        }
        self.derived_nodes = total;
        Ok(())
    }

    pub fn parse(&mut self, uri: &str, raml: bool) -> Result<Value> {
        if let Some(source) = self.documents.get(uri) {
            return Ok(source.document.clone());
        }
        let text = self.text(uri)?.to_owned();
        let SyntaxDocument { source, includes } = if raml {
            octri_parser::parse_raml_source(uri, text, &self.limits.parser())?
        } else {
            SyntaxDocument {
                source: octri_parser::parse_source(uri, text, &self.limits.parser())?,
                includes: BTreeMap::new(),
            }
        };
        let value = source.document.clone();
        self.remember(source)?;
        for (pointer, path) in includes {
            self.includes.insert(
                Address {
                    uri: uri.into(),
                    pointer,
                },
                path,
            );
        }
        Ok(value)
    }

    pub fn remember(&mut self, source: SourceDocument) -> Result<()> {
        if self.documents.contains_key(&source.uri) {
            return Ok(());
        }
        let mut queue = vec![&source.document];
        let mut count = 0usize;
        while let Some(value) = queue.pop() {
            count += 1;
            if self.parsed_nodes.saturating_add(count) > self.limits.max_nodes {
                return Err(ConversionError::new(
                    "LIMIT_NODES",
                    "Combined source documents exceed configured node budget",
                ));
            }
            match value {
                Value::Object(map) => queue.extend(map.values()),
                Value::Array(items) => queue.extend(items),
                _ => {}
            }
        }
        self.parsed_nodes += count;
        self.documents.insert(source.uri.clone(), source);
        Ok(())
    }

    /// Locate an address already known to belong to original source bytes.
    /// Logical aliases used during conversion must not remap it a second time.
    pub fn physical_location(&self, address: &Address) -> Location {
        self.documents.get(&address.uri).map_or_else(
            || address.clone().into(),
            |source| source.locate(&address.pointer),
        )
    }

    pub fn location(&self, uri: &str, pointer: &str) -> Location {
        let address = Address {
            uri: uri.into(),
            pointer: pointer.into(),
        };
        if let Some(location) = self.text_locations.get(&address) {
            return location.clone();
        }
        let mut parent = pointer;
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            if let Some(location) = self.text_locations.get(&Address {
                uri: uri.into(),
                pointer: prefix.into(),
            }) {
                let mut location = location.clone();
                location.address.pointer.push_str(&pointer[prefix.len()..]);
                return location;
            }
            parent = prefix;
        }
        if uri == self.root
            && let Some(origin) = self.origins.get(pointer)
        {
            return self.documents.get(&origin.uri).map_or_else(
                || origin.clone().into(),
                |source| source.locate(&origin.pointer),
            );
        }
        self.documents.get(uri).map_or_else(
            || {
                Address {
                    uri: uri.into(),
                    pointer: pointer.into(),
                }
                .into()
            },
            |source| source.locate(pointer),
        )
    }

    pub fn register_text_location(&mut self, uri: &str, pointer: &str, line: usize, column: usize) {
        let offset = self.files.get(uri).and_then(|text| {
            let start = text
                .split_inclusive('\n')
                .take(line.saturating_sub(1))
                .map(str::len)
                .sum::<usize>();
            let line_text = text.get(start..)?.split('\n').next()?;
            let column_offset = line_text
                .char_indices()
                .nth(column.saturating_sub(1))
                .map_or(line_text.len(), |(offset, _)| offset);
            Some(start + column_offset)
        });
        let address = Address {
            uri: uri.into(),
            pointer: pointer.into(),
        };
        self.text_locations.insert(
            address.clone(),
            Location {
                address,
                line: Some(line),
                column: Some(column),
                offset,
            },
        );
    }

    pub fn register_byte_location(
        &mut self,
        uri: &str,
        pointer: &str,
        offset: usize,
        line: usize,
        column: usize,
    ) {
        let address = Address {
            uri: uri.into(),
            pointer: pointer.into(),
        };
        self.text_locations.insert(
            address.clone(),
            Location {
                address,
                line: Some(line),
                column: Some(column),
                offset: Some(offset),
            },
        );
    }

    pub fn register_model_nodes(&mut self, nodes: usize) -> Result<()> {
        self.parsed_nodes = self.parsed_nodes.saturating_add(nodes);
        if self.parsed_nodes > self.limits.max_nodes {
            return Err(ConversionError::new(
                "LIMIT_NODES",
                "Combined source models exceed configured node budget",
            ));
        }
        Ok(())
    }

    pub fn register_text_origin(
        &mut self,
        logical: Address,
        physical: Address,
        line: usize,
        column: usize,
    ) {
        self.register_text_location(&physical.uri, &physical.pointer, line, column);
        if let Some(location) = self.text_locations.get(&physical).cloned() {
            self.text_locations.insert(logical, location);
        }
    }

    pub fn manifest(&self) -> Vec<SourceFile> {
        self.files
            .iter()
            .map(|(uri, text)| SourceFile {
                uri: uri.clone(),
                sha256: hash(text.as_bytes()),
                bytes: text.len(),
            })
            .collect()
    }

    pub fn bundle_openapi(&mut self) -> Result<Value> {
        let input = octri_parser::Input {
            uri: self.root.clone(),
            content: Some(self.text(&self.root)?.into()),
            generated: false,
        };
        let options = octri_parser::Options {
            files: self.files.clone(),
            limits: self.limits.parser(),
            ..Default::default()
        };
        let bundled = octri_parser::bundle_sources(input, options, &mut octri_parser::NoIo)?;
        self.schema_references = bundled.references;
        for source in bundled.sources {
            self.remember(source)?;
        }
        self.origins = bundled.origins;
        Ok(bundled.document)
    }

    pub fn relative(&self, from: &str, path: &str) -> Result<String> {
        let base = Url::parse(from)
            .map_err(|error| ConversionError::new("INVALID_URI", error.to_string()))?;
        let uri = base
            .join(path)
            .map_err(|error| ConversionError::new("INVALID_URI", error.to_string()))?;
        canonical_uri(uri.as_str())
    }

    /// Serialized JSON schemas inside foreign strings need a parser-visible
    /// document. Derived documents stay outside the original source manifest;
    /// their locations map back to the authored scalar instead of replacing it.
    pub fn bundle_derived_schemas(
        &mut self,
        envelope: Value,
        roots: Vec<octri_parser::SchemaRoot>,
        registry: Vec<octri_parser::SchemaRoot>,
        derived: BTreeMap<String, (Value, Address)>,
    ) -> Result<Value> {
        // Generated envelope must not replace a physical source retrieval URI.
        let mut uri = "memory://octri-converter-internal/schema-envelope.json".to_owned();
        while self.files.contains_key(&uri) {
            uri.push('_');
        }
        let content = envelope.to_string();
        let mut limits = self.generated_limits(&envelope, &content);
        let mut files = self.files.clone();
        let mut derived_nodes = 0usize;
        let mut derived_bytes = 0usize;
        for (uri, (document, _)) in &derived {
            if files.contains_key(uri) {
                return Err(ConversionError::new(
                    "SCHEMA_DERIVED_URI",
                    "Derived schema identity collides with a supplied source",
                ));
            }
            let text = document.to_string();
            derived_bytes = derived_bytes.saturating_add(text.len());
            let mut nodes = vec![document];
            while let Some(value) = nodes.pop() {
                derived_nodes = derived_nodes.saturating_add(1);
                match value {
                    Value::Object(map) => nodes.extend(map.values()),
                    Value::Array(values) => nodes.extend(values),
                    _ => {}
                }
            }
            if derived.len() > self.limits.max_documents
                || derived_nodes > self.limits.max_nodes
                || derived_bytes > self.limits.max_total_bytes
            {
                return Err(ConversionError::new(
                    "SCHEMA_DERIVED_LIMIT",
                    "Embedded schemas exceed configured document, node or byte limits",
                ));
            }
            limits.max_document_bytes = limits.max_document_bytes.max(text.len());
            files.insert(uri.clone(), text);
        }
        limits.max_documents = limits.max_documents.saturating_add(derived.len());
        limits.max_nodes = limits.max_nodes.saturating_add(derived_nodes);
        limits.max_total_bytes = limits.max_total_bytes.saturating_add(derived_bytes);
        let bundled = octri_parser::bundle_schema_registry_with_dialects(
            octri_parser::Input {
                uri: uri.clone(),
                content: Some(content),
                generated: true,
            },
            roots,
            registry,
            octri_parser::Options {
                files,
                limits,
                ..Default::default()
            },
            &mut octri_parser::NoIo,
        )
        .map_err(|mut error| {
            for diagnostic in &mut error.diagnostics {
                for location in
                    std::iter::once(&mut diagnostic.location).chain(diagnostic.related.iter_mut())
                {
                    if let Some((_, physical)) = derived.get(&location.address.uri) {
                        *location = self.physical_location(physical);
                    }
                }
            }
            error
        })?;
        let mut references = bundled.references;
        for edge in &mut references {
            for location in std::iter::once(&mut edge.from).chain(edge.target.iter_mut()) {
                if let Some((_, physical)) = derived.get(&location.address.uri) {
                    *location = self.physical_location(physical);
                }
            }
        }
        self.schema_references = references;
        for source in bundled.sources {
            if source.uri != uri && !derived.contains_key(&source.uri) {
                self.remember(source)?;
            }
        }
        for (pointer, origin) in bundled.origins {
            let origin = if let Some((_, scalar)) = derived.get(&origin.uri) {
                // An embedded string has one physical scalar location; invented
                // offsets inside its decoded JSON would be misleading.
                scalar.clone()
            } else {
                origin
            };
            if origin.uri != uri {
                self.origins.insert(pointer, origin);
            }
        }
        Ok(bundled.document)
    }

    /// Internal documents have a separate, exact allowance. Admission still
    /// checks physical file/byte caps in `new`, and `remember` checks physical
    /// node totals. Final serialization checks output limits independently.
    pub fn generated_limits(&self, document: &Value, content: &str) -> octri_parser::Limits {
        let mut limits = self.limits.parser();
        limits.max_documents += 1;
        limits.max_document_bytes = limits.max_document_bytes.max(content.len());
        limits.max_total_bytes += content.len();
        let mut queue = vec![document];
        while let Some(value) = queue.pop() {
            limits.max_nodes += 1;
            match value {
                Value::Object(map) => queue.extend(map.values()),
                Value::Array(values) => queue.extend(values),
                _ => {}
            }
        }
        limits
    }
}

fn derived_limit() -> ConversionError {
    ConversionError::new(
        "SCHEMA_DERIVED_LIMIT",
        "Embedded schemas exceed configured document, node or byte limits",
    )
}

fn canonical_uri(raw: &str) -> Result<String> {
    let mut uri =
        Url::parse(raw).map_err(|error| ConversionError::new("INVALID_URI", error.to_string()))?;
    if uri.username() != "" || uri.password().is_some() || uri.fragment().is_some() {
        return Err(ConversionError::new(
            "INVALID_URI",
            "Source URIs cannot contain credentials or fragments",
        ));
    }
    if !matches!(uri.scheme(), "memory" | "file" | "http" | "https") {
        return Err(ConversionError::new(
            "INVALID_URI",
            "Use a memory, file, HTTP or HTTPS source identifier",
        ));
    }
    uri.set_fragment(None);
    Ok(uri.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, ConversionRequest};

    #[test]
    fn derived_byte_admission_is_shared_atomic_and_reuses_identity() -> Result<()> {
        let mut sources = Sources::new(&ConversionRequest {
            input: octri_parser::Input {
                uri: "memory://limits/api.json".into(),
                content: Some("{}".into()),
                generated: false,
            },
            files: Default::default(),
            config: Config {
                limits: Limits {
                    max_total_bytes: 7,
                    ..Default::default()
                },
                ..Default::default()
            },
        })?;
        sources.admit_derived("json:one", 4, 2)?;
        sources.admit_derived("json:one", 4, 2)?;
        assert_eq!(
            sources
                .admit_derived("xml:two", 4, 2)
                .err()
                .ok_or_else(|| ConversionError::new("TEST_ADMISSION", "Mixed bytes accepted"))?
                .code,
            "SCHEMA_DERIVED_LIMIT"
        );
        // Failed admission consumes no identity or capacity. Exact boundary fits.
        sources.admit_derived("xml:two", 3, 2)?;
        assert_eq!(sources.derived_resources.len(), 2);
        assert_eq!(sources.derived_bytes, 7);
        assert_eq!(sources.derived_nodes, 4);
        Ok(())
    }
}
