//! Versioned public contracts. Every surface serializes these same types.

use octri_parser::{Diagnostic, Input, Location};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPORT_VERSION: &str = "1.0.0";
pub const MAX_ENVELOPE_BYTES: usize = 128 * 1024 * 1024;
pub const TARGET_VERSIONS: &[&str] = &[
    "2.0", "3.0.0", "3.0.1", "3.0.2", "3.0.3", "3.0.4", "3.1.0", "3.1.1", "3.1.2", "3.2.0", "3.2.1",
];

#[derive(Debug, Clone, Default, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputFormat {
    #[default]
    Auto,
    Openapi,
    Swagger,
    Postman,
    Raml,
    Insomnia,
    Har,
    Curl,
    Asyncapi,
    Graphql,
    Protobuf,
    Wsdl,
}

#[derive(Debug, Clone, Default, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Json,
    Yaml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Complete,
    NeedsReview,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Classification {
    Preserved,
    Transformed,
    Inferred,
    Omitted,
    Blocked,
}

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
    pub max_decisions: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_document_bytes: 5 * 1024 * 1024,
            max_total_bytes: 20 * 1024 * 1024,
            max_documents: 100,
            max_nodes: 1_000_000,
            max_depth: 128,
            max_aliases: 64,
            max_output_bytes: 64 * 1024 * 1024,
            max_decisions: 10_000,
        }
    }
}

impl Limits {
    pub(crate) fn parser(&self) -> octri_parser::Limits {
        octri_parser::Limits {
            max_document_bytes: self.max_document_bytes,
            max_total_bytes: self.max_total_bytes,
            max_documents: self.max_documents,
            max_nodes: self.max_nodes,
            max_depth: self.max_depth,
            max_aliases: self.max_aliases,
            max_output_bytes: self.max_output_bytes,
            max_diagnostics: self.max_decisions.min(10_000),
        }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let ceiling = Self::default();
        let fields = [
            (self.max_document_bytes, ceiling.max_document_bytes),
            (self.max_total_bytes, ceiling.max_total_bytes),
            (self.max_documents, ceiling.max_documents),
            (self.max_nodes, ceiling.max_nodes),
            (self.max_depth, ceiling.max_depth),
            (self.max_output_bytes, ceiling.max_output_bytes),
            (self.max_decisions, ceiling.max_decisions),
        ];
        if fields.iter().any(|(value, max)| *value == 0 || value > max)
            || self.max_aliases > ceiling.max_aliases
        {
            return Err(ConversionError::new(
                "INVALID_LIMITS",
                "Limits may lower, but never exceed, the published safety ceilings",
            ));
        }
        Ok(())
    }
}

/// A caller-authored HTTP binding for a source operation in another protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpMapping {
    pub method: String,
    pub path: String,
    pub server: String,
    pub response_status: String,
    #[serde(default)]
    pub request_schema: Option<Value>,
    #[serde(default)]
    pub response_schema: Option<Value>,
    #[serde(default)]
    pub security: Option<Value>,
    #[serde(default)]
    pub parameter_bindings: BTreeMap<String, ParameterBinding>,
    #[serde(default)]
    pub payload_location: Option<String>,
    #[serde(default)]
    pub request_stream: Option<StreamBinding>,
    #[serde(default)]
    pub response_stream: Option<StreamBinding>,
}

/// An authored HTTP stream contract. Conversion describes this gateway; it does
/// not implement the source protocol or infer its cancellation/error behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamBinding {
    pub framing: StreamFraming,
    /// Describe completion, cancellation, errors and reconnection/resumption.
    pub lifecycle: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StreamFraming {
    Ndjson,
    JsonSequence,
    Sse,
}

/// Structured input for review surfaces. Never recover configuration keys by
/// parsing diagnostic prose; operation identifiers are protocol-specific.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingRequest {
    pub key: String,
    pub source: Location,
    pub input_fields: Vec<String>,
    pub has_input: bool,
    pub has_output: bool,
    pub request_stream_required: bool,
    pub response_stream_required: bool,
    pub payload_location_required: bool,
}

/// An authored credential choice is valid only for these exact source bytes
/// and inherited auth scope. It never contains credential values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialSelection {
    pub source_uri: String,
    pub source_sha256: String,
    pub auth_pointer: String,
    pub credential_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialChoice {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    pub auth_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSelectionRequest {
    pub name: String,
    pub request: Location,
    pub auth_source: Location,
    pub source_sha256: String,
    pub choices: Vec<CredentialChoice>,
    pub selected: Option<usize>,
}

/// A source pointer remains the stable configuration key even when its
/// physical location is remapped by a source adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateCandidate {
    pub pointer: String,
    pub source: Location,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateRequest {
    pub identity: String,
    pub candidates: Vec<DuplicateCandidate>,
    pub selected: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParameterBinding {
    pub location: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    pub input_format: InputFormat,
    /// None preserves existing OpenAPI versions; foreign formats use 3.1.2.
    pub target_version: Option<String>,
    pub output_format: OutputFormat,
    pub bundle: bool,
    pub title: Option<String>,
    pub api_version: Option<String>,
    pub base_url: Option<String>,
    pub environment_uri: Option<String>,
    /// Select an Insomnia sub-environment by its exported identifier.
    pub environment_id: Option<String>,
    pub variables: BTreeMap<String, String>,
    pub http_mappings: BTreeMap<String, HttpMapping>,
    /// Physical request pointer -> a source-bound credential-set choice.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub credential_selections: BTreeMap<String, CredentialSelection>,
    pub security_schemes: BTreeMap<String, Value>,
    /// Duplicate method/path -> the explicitly selected source pointer.
    pub duplicate_choices: BTreeMap<String, String>,
    /// Output JSON pointers to caller-authored replacement values.
    pub overrides: BTreeMap<String, Value>,
    /// Stable review IDs. Acknowledgement never overrides a blocked decision.
    pub accepted_decisions: BTreeSet<String>,
    /// Individual output keyword pointers explicitly approved for lossy removal.
    pub drop_unsupported: BTreeSet<String>,
    pub limits: Limits,
}

impl Config {
    pub(crate) fn canonicalized(&self) -> Self {
        let mut config = self.clone();
        for value in config
            .security_schemes
            .values_mut()
            .chain(config.overrides.values_mut())
        {
            octri_parser::numbers::canonicalize(value);
        }
        for mapping in config.http_mappings.values_mut() {
            for value in [
                &mut mapping.request_schema,
                &mut mapping.response_schema,
                &mut mapping.security,
            ]
            .into_iter()
            .flatten()
            {
                octri_parser::numbers::canonicalize(value);
            }
        }
        config
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionRequest {
    pub input: Input,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub config: Config,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub id: String,
    pub code: String,
    pub classification: Classification,
    pub source: Location,
    pub output_pointers: Vec<String>,
    pub message: String,
    pub remediation: Option<String>,
    pub review_required: bool,
    pub reviewed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFile {
    pub uri: String,
    pub sha256: String,
    pub bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub engine_version: String,
    pub parser_version: String,
    pub processing_mode: String,
    pub root: String,
    pub source_format: InputFormat,
    pub target_version: String,
    pub files: Vec<SourceFile>,
    pub config: Config,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputFile {
    pub uri: String,
    pub content: String,
    pub sha256: String,
    pub encoding: OutputFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub id: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionReport {
    pub schema_version: String,
    pub manifest: Manifest,
    pub status: Status,
    pub valid: bool,
    pub handoff_allowed: bool,
    pub output: Option<OutputFile>,
    pub document: Option<Value>,
    #[serde(default)]
    pub inspection: Option<octri_parser::Inspection>,
    pub decisions: Vec<Decision>,
    #[serde(default)]
    pub mapping_requests: Vec<MappingRequest>,
    #[serde(default)]
    pub duplicate_requests: Vec<DuplicateRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_selection_requests: Vec<CredentialSelectionRequest>,
    pub diagnostics: Vec<Diagnostic>,
    pub checks: Vec<Check>,
    pub audit: Option<octri_auditor::AuditReport>,
    pub original: ConversionRequest,
}

#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[error("{message}")]
pub struct ConversionError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

impl ConversionError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            diagnostics: Vec::new(),
        }
    }
}

impl From<octri_parser::ParserError> for ConversionError {
    fn from(error: octri_parser::ParserError) -> Self {
        Self {
            code: "SOURCE_INVALID".into(),
            message: error.message,
            diagnostics: error.diagnostics,
        }
    }
}

pub type Result<T> = std::result::Result<T, ConversionError>;
