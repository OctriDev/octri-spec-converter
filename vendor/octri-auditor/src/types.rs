//! Versioned public contracts. Language adapters transport these types unchanged.

use octri_parser::{Diagnostic, Dialect, Input, Location};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const RULESET_VERSION: &str = "1.0.2";
pub const REPORT_VERSION: &str = "1.0.0";
pub const MAX_ENVELOPE_BYTES: usize = 128 * 1024 * 1024;

// Computed scores are bounded binary floats, unlike user-authored JSON numbers.
// Decode through Value so internally tagged transport enums preserve serde_json's
// arbitrary-precision number representation rather than exposing its private map.
fn finite_score<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> std::result::Result<f64, D::Error> {
    let value = Value::deserialize(decoder)?;
    score_value(&value)
        .ok_or_else(|| serde::de::Error::custom("Expected an exactly representable finite score"))
}

fn score_value(value: &Value) -> Option<f64> {
    let original = value.as_number()?;
    let score = original.as_f64().filter(|score| score.is_finite())?;
    let encoded = serde_json::Number::from_f64(score)?;
    (octri_parser::numbers::compare(original, &encoded) == std::cmp::Ordering::Equal)
        .then_some(score)
}

fn optional_finite_score<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> std::result::Result<Option<f64>, D::Error> {
    let value = Value::deserialize(decoder)?;
    if value.is_null() {
        return Ok(None);
    }
    score_value(&value)
        .map(Some)
        .ok_or_else(|| serde::de::Error::custom("Expected a finite numeric score or null"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuditRequest {
    pub input: Input,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub config: Config,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    /// Root encoding override; external resources keep their own URI hints.
    pub root_format: octri_parser::SourceFormat,
    pub limits: AuditLimits,
    pub profiles: Vec<String>,
    pub rules: BTreeMap<String, RuleSetting>,
    pub ignored: Vec<String>,
    pub exclusions: Vec<Exclusion>,
    pub baseline: Option<Baseline>,
    pub fail_on_new: bool,
    pub fail_on: Severity,
    /// A caller-supplied date makes expiring exclusions deterministic in CI.
    pub as_of: Option<String>,
    pub max_findings: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            root_format: octri_parser::SourceFormat::Auto,
            limits: AuditLimits::default(),
            profiles: vec!["docs".into(), "sdk".into(), "mcp".into()],
            rules: BTreeMap::new(),
            ignored: Vec::new(),
            exclusions: Vec::new(),
            baseline: None,
            fail_on_new: false,
            fail_on: Severity::High,
            as_of: None,
            max_findings: 5000,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleSetting {
    pub enabled: Option<bool>,
    pub severity: Option<Severity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct AuditLimits {
    pub max_document_bytes: usize,
    pub max_total_bytes: usize,
    pub max_documents: usize,
    pub max_nodes: usize,
    pub max_depth: usize,
}
impl Default for AuditLimits {
    fn default() -> Self {
        Self {
            max_document_bytes: 5 * 1024 * 1024,
            max_total_bytes: 20 * 1024 * 1024,
            max_documents: 100,
            max_nodes: 300_000,
            max_depth: 64,
        }
    }
}
impl AuditLimits {
    pub fn parser(&self) -> octri_parser::Limits {
        octri_parser::Limits {
            max_document_bytes: self.max_document_bytes,
            max_total_bytes: self.max_total_bytes,
            max_documents: self.max_documents,
            max_nodes: self.max_nodes,
            max_depth: self.max_depth,
            max_aliases: 64,
            max_output_bytes: 64 * 1024 * 1024,
            max_diagnostics: 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Exclusion {
    pub rule_id: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub pointer: Option<String>,
    pub reason: String,
    #[serde(default)]
    pub expires_on: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Baseline {
    pub schema_version: String,
    pub ruleset_version: String,
    pub fingerprints: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Advice,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Integrity,
    Docs,
    Sdk,
    Mcp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub key: String,
    pub fingerprint: String,
    pub rule_id: String,
    pub title: String,
    pub why: String,
    pub severity: Severity,
    pub target: String,
    pub message: String,
    pub fixable: bool,
    pub ignorable: bool,
    pub location: Location,
    pub related: Vec<Location>,
    pub categories: Vec<Category>,
    pub impact: String,
    pub evidence: String,
    pub remediation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<ProposedFix>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusion_reason: Option<String>,
    pub baseline: bool,
}

/// A proposal is not an applied edit. Exact before-values guard against stale UI state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProposedFix {
    pub label: String,
    pub kind: String,
    #[serde(default)]
    pub edits: Vec<Edit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Edit {
    pub uri: String,
    pub pointer: String,
    pub before: Option<Value>,
    pub after: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleResult {
    pub rule_id: String,
    pub title: String,
    #[serde(deserialize_with = "finite_score")]
    pub weight: f64,
    pub findings: usize,
    #[serde(deserialize_with = "finite_score")]
    pub deduction: f64,
    pub denominator: usize,
    pub status: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessView {
    pub category: Category,
    pub status: String,
    #[serde(deserialize_with = "optional_finite_score")]
    pub score: Option<f64>,
    pub blockers: usize,
    pub checked: usize,
    pub failed: usize,
    pub skipped: usize,
    pub unsupported: usize,
    pub findings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileResult {
    pub id: String,
    pub version: String,
    pub evidence: String,
    pub executed_verification: String,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub engine_version: String,
    pub ruleset_version: String,
    pub score_method: String,
    pub processing_mode: String,
    pub root: String,
    pub files: Vec<ArtifactFile>,
    pub config: Config,
    pub profiles: Vec<ProfileResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactFile {
    pub uri: String,
    pub sha256: String,
    pub bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Surface {
    pub operations: usize,
    pub models: usize,
    pub paths: usize,
    #[serde(default)]
    pub schemas: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditReport {
    pub schema_version: String,
    pub manifest: Manifest,
    pub valid: bool,
    pub complete: bool,
    pub status: String,
    pub dialect: Option<Dialect>,
    #[serde(deserialize_with = "finite_score")]
    pub score: f64,
    #[serde(deserialize_with = "finite_score")]
    pub max_score: f64,
    pub findings: Vec<Finding>,
    pub ignored_findings: Vec<Finding>,
    pub applied_findings: Vec<Finding>,
    pub by_rule: Vec<RuleResult>,
    pub surface: Surface,
    pub diagnostics: Vec<Diagnostic>,
    pub views: Vec<ReadinessView>,
    pub policy_passed: bool,
    pub coverage: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct AuditError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

impl AuditError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            diagnostics: Vec::new(),
        }
    }
}

pub type Result<T> = std::result::Result<T, AuditError>;
