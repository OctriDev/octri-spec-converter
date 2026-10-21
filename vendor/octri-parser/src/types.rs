//! Public request, result and diagnostic types shared by every adapter.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dialect {
    #[serde(rename = "swagger2")]
    Swagger2,
    #[serde(rename = "openapi3.0")]
    OpenApi30,
    #[serde(rename = "openapi3.1")]
    OpenApi31,
    #[serde(rename = "openapi3.2")]
    OpenApi32,
}

impl Dialect {
    pub fn modern_schema(self) -> bool {
        matches!(self, Self::OpenApi31 | Self::OpenApi32)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Address {
    pub uri: String,
    pub pointer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    #[serde(flatten)]
    pub address: Address,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

impl From<Address> for Location {
    fn from(address: Address) -> Self {
        Self {
            address,
            line: None,
            column: None,
            offset: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub location: Location,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<Location>,
}

#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[error("{message}")]
pub struct ParserError {
    pub message: String,
    pub diagnostics: Vec<Diagnostic>,
}

impl ParserError {
    pub fn new(code: &str, message: impl Into<String>, address: Address) -> Self {
        let message = message.into();
        Self {
            message: message.clone(),
            diagnostics: vec![Diagnostic {
                code: code.into(),
                severity: Severity::Error,
                message,
                location: address.into(),
                related: Vec::new(),
            }],
        }
    }
}

pub type Result<T> = std::result::Result<T, ParserError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Limits {
    pub max_document_bytes: usize,
    pub max_total_bytes: usize,
    pub max_documents: usize,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_aliases: usize,
    pub max_output_bytes: usize,
    pub max_diagnostics: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_document_bytes: 16 * 1024 * 1024,
            max_total_bytes: 64 * 1024 * 1024,
            max_documents: 128,
            max_nodes: 1_000_000,
            max_depth: 128,
            max_aliases: 64,
            max_output_bytes: 64 * 1024 * 1024,
            max_diagnostics: 1000,
        }
    }
}

impl Limits {
    pub(crate) fn check(&self) -> Result<()> {
        let valid = self.max_depth > 0
            && self.max_depth <= 128
            && self.max_nodes > 0
            && self.max_documents > 0
            && self.max_document_bytes > 0
            && self.max_total_bytes > 0
            && self.max_output_bytes > 0
            && self.max_diagnostics > 0;
        if !valid {
            return Err(ParserError::new(
                "INVALID_OPTIONS",
                "Limits must be positive; maxDepth cannot exceed 128",
                Address::default(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CircularPolicy {
    #[default]
    Preserve,
    Ignore,
    Error,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct DereferenceOptions {
    pub circular: CircularPolicy,
    pub clone_references: bool,
}

/// Compatible validation accepts generator-repairable documents. Strict mode
/// adds target-shape, JSON Schema keyword and audit semantic checks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValidationMode {
    #[default]
    Compatible,
    Strict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ValidationOptions {
    pub mode: ValidationMode,
    pub schema: bool,
    pub spec: bool,
}
impl Default for ValidationOptions {
    fn default() -> Self {
        Self {
            mode: ValidationMode::Compatible,
            schema: true,
            spec: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ResolveOptions {
    pub external: bool,
    /// No implicit file permission follows from a content input's base URI.
    pub file_root: Option<String>,
    pub http: bool,
    pub allow_hosts: Vec<String>,
    pub allow_private: bool,
    pub timeout_ms: u64,
    pub max_redirects: usize,
}
impl Default for ResolveOptions {
    fn default() -> Self {
        Self {
            external: true,
            file_root: None,
            http: false,
            allow_hosts: Vec::new(),
            allow_private: false,
            timeout_ms: 15_000,
            max_redirects: 5,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Options {
    /// Explicit encoding for the root only. Auto retains retrieval-URI hints.
    pub root_format: SourceFormat,
    pub files: BTreeMap<String, String>,
    /// Checked retrieval redirects supplied by the host. Files use final URI keys.
    /// This metadata grants no network or filesystem authority.
    pub retrieval_aliases: BTreeMap<String, String>,
    pub limits: Limits,
    pub resolve: ResolveOptions,
    pub validate: ValidationOptions,
    pub dereference: DereferenceOptions,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceFormat {
    #[default]
    Auto,
    Json,
    Yaml,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub uri: String,
    /// Absent content requests a host resolver read. Core never reads by itself.
    pub content: Option<String>,
    /// Generated object transport has pointers, but no original text positions.
    #[serde(default)]
    pub generated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Parse,
    Resolve,
    Bundle,
    Dereference,
    Validate,
    Inspect,
    Analyze,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub operation: Operation,
    pub input: Input,
    #[serde(default)]
    pub options: Options,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceDocument {
    pub uri: String,
    pub text: String,
    pub document: Value,
    pub locations: BTreeMap<String, Location>,
    pub bytes: usize,
    pub generated: bool,
}

/// A syntax document with inert RAML include declarations. Loading an include
/// remains the caller's job; parsing never opens a path or URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyntaxDocument {
    pub source: SourceDocument,
    pub includes: BTreeMap<String, String>,
}

/// A portable reference bundle and the physical origin of every copied value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundledSource {
    pub document: Value,
    pub origins: BTreeMap<String, Address>,
    pub sources: Vec<SourceDocument>,
    pub diagnostics: Vec<Diagnostic>,
    /// Resolved physical edges, before bundle aliases and resource relocation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<ReferenceEdge>,
}
impl SourceDocument {
    pub fn locate(&self, pointer: &str) -> Location {
        let mut parent = pointer;
        loop {
            if let Some(location) = self.locations.get(parent) {
                let mut location = location.clone();
                location.address.pointer = pointer.into();
                return location;
            }
            match parent.rsplit_once('/') {
                Some((p, _)) => parent = p,
                None => break,
            }
        }
        Address {
            uri: self.uri.clone(),
            pointer: pointer.into(),
        }
        .into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceEdge {
    pub from: Location,
    #[serde(rename = "ref")]
    pub reference: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Location>,
    pub resolved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiOperation {
    /// Fixed OpenAPI fields use their standard uppercase method. Keys from
    /// additionalOperations retain their exact, case-sensitive wire spelling.
    pub method: String,
    pub path: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Extension values remain literal data at the operation's physical source.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
    pub tags: Vec<String>,
    pub deprecated: bool,
    pub location: Location,
    pub parameters: Vec<Value>,
    pub security: Vec<Value>,
    pub servers: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body: Option<Value>,
    pub responses: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaEntry {
    pub name: String,
    pub location: Location,
    pub schema: Value,
    pub used_by: Vec<Location>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Statistics {
    pub operations: usize,
    pub paths: usize,
    pub schemas: usize,
    pub documents: usize,
    pub references: usize,
    pub unresolved: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub dialect: Dialect,
    pub version: String,
    pub info: Value,
    pub operations: Vec<ApiOperation>,
    pub schemas: Vec<SchemaEntry>,
    pub security_schemes: Value,
    pub references: Vec<ReferenceEdge>,
    pub statistics: Statistics,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceIndex {
    pub root_uri: String,
    /// Logical input after defaults, present only when it differs from sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_document: Option<Value>,
    pub resources: BTreeMap<String, Address>,
    pub anchors: BTreeMap<String, Address>,
    /// Retrieval redirects, separate from authored schema resource identifiers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub retrieval_aliases: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub valid: bool,
    pub dialect: Option<Dialect>,
    pub document: Option<Value>,
    pub diagnostics: Vec<Diagnostic>,
    pub sources: Vec<SourceDocument>,
    pub references: Vec<ReferenceEdge>,
    pub registry: ReferenceIndex,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inspection: Option<Inspection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph: Option<crate::Graph>,
}
