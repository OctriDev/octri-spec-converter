//! Octri's existing HTTP contract. It is a projection over the same Rust engine.
//! Stored manual suggestions identify known fields; they never supply executable
//! patches or bypass source-before checks and integrity revalidation.

use crate::{
    AuditError, AuditRequest, Config, Edit, Finding, Input, Result, audit, catalogue, fixes,
    source::Sources,
};
use octri_parser::{NoIo, Operation, Options, Request, ValidationMode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyFixRequest {
    pub schema: String,
    pub key: String,
    #[serde(default)]
    pub input: Option<Value>,
    #[serde(default)]
    pub ignored: Vec<String>,
    #[serde(default)]
    pub applied_finding: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyFixResult {
    pub schema: String,
    pub audit: crate::AuditReport,
    pub applied_key: String,
    pub applied_finding: Finding,
}

pub fn fix(input: LegacyFixRequest) -> Result<LegacyFixResult> {
    let request = AuditRequest {
        input: Input {
            uri: "memory://audit/openapi.yaml".into(),
            content: Some(input.schema),
            generated: false,
        },
        files: BTreeMap::new(),
        config: Config {
            ignored: input
                .ignored
                .into_iter()
                .filter(|key| key != &input.key)
                .collect(),
            ..Config::default()
        },
    };
    let mut request = request;
    request.config.limits = crate::AuditLimits {
        max_document_bytes: 24 * 1024 * 1024,
        max_total_bytes: 24 * 1024 * 1024,
        max_nodes: 1_000_000,
        max_depth: 128,
        ..crate::AuditLimits::default()
    };
    let report = audit(&request)?;
    if let Some(finding) = report
        .findings
        .iter()
        .find(|finding| finding.key == input.key)
        .cloned()
    {
        let mut inputs = BTreeMap::new();
        if let Some(value) = input.input {
            inputs.insert(input.key.clone(), value);
        }
        let result = fixes::apply(&fixes::FixRequest {
            request,
            keys: vec![input.key.clone()],
            expected_files: fixes::hashes(&report),
            inputs,
        })?;
        return Ok(LegacyFixResult {
            schema: result.revised.input.content.unwrap_or_default(),
            audit: result.report,
            applied_key: input.key,
            applied_finding: finding,
        });
    }
    let stored = input.applied_finding.ok_or_else(|| {
        AuditError::new(
            "FINDING_UNAVAILABLE",
            "Finding is no longer available; run the audit again",
        )
    })?;
    if stored.get("key").and_then(Value::as_str) != Some(input.key.as_str()) {
        return Err(AuditError::new(
            "FINDING_UNAVAILABLE",
            "Stored finding does not match this selection",
        ));
    }
    let rule_id = stored
        .get("ruleId")
        .and_then(Value::as_str)
        .ok_or_else(|| AuditError::new("INVALID_STORED_FINDING", "Stored rule is missing"))?;
    if !input.key.starts_with(&format!("{rule_id}:")) {
        return Err(AuditError::new(
            "INVALID_STORED_FINDING",
            "Stored key and rule disagree",
        ));
    }
    let metadata = &stored["fix"];
    let kind = metadata["kind"].as_str().unwrap_or("");
    if !matches!(kind, "text" | "response-schema") {
        return Err(AuditError::new(
            "FINDING_UNAVAILABLE",
            "Only authored text and schema suggestions can be edited after application",
        ));
    }
    let mut options = Options::default();
    options.validate.mode = ValidationMode::Strict;
    let parsed = octri_parser::execute(
        Request {
            operation: Operation::Analyze,
            input: request.input.clone(),
            options,
        },
        &mut NoIo,
    )
    .map_err(|error| AuditError::new("PATCH_PARSE", error.message))?;
    let sources = Sources(&parsed);
    let target = input.key.strip_prefix(&format!("{rule_id}:")).unwrap_or("");
    let operation = parsed
        .inspection
        .as_ref()
        .and_then(|inventory| {
            inventory.operations.iter().find(|operation| {
                let prefix = format!("{} {}", operation.method, operation.path);
                target == prefix || target.starts_with(&format!("{prefix}#"))
            })
        })
        .ok_or_else(|| AuditError::new("PATCH_TARGET", "Stored operation no longer exists"))?;
    let value = input.input.ok_or_else(|| {
        AuditError::new(
            "AUTHORED_INPUT_REQUIRED",
            "Provide the actual description or response schema",
        )
    })?;
    let (at, after) = match (rule_id, kind) {
        ("operation-documented", "text") => {
            let at = sources.field(&operation.location.address, "summary");
            (at, description(&value)?)
        }
        ("request-body-documented", "text") => {
            let body = sources.follow(&sources.field(&operation.location.address, "requestBody"));
            if sources.value(&body).is_none() {
                return Err(AuditError::new(
                    "PATCH_TARGET",
                    "Request body no longer exists",
                ));
            }
            (sources.field(&body, "description"), description(&value)?)
        }
        ("parameters-documented", "text") => {
            let prefix = format!("{} {}#", operation.method, operation.path);
            let (place, name) = target
                .strip_prefix(&prefix)
                .and_then(|fragment| fragment.split_once(':'))
                .ok_or_else(|| AuditError::new("PATCH_TARGET", "Invalid parameter selection"))?;
            if !operation
                .parameters
                .iter()
                .any(|parameter| parameter["name"] == name && parameter["in"] == place)
            {
                return Err(AuditError::new(
                    "PATCH_TARGET",
                    "Parameter no longer exists",
                ));
            }
            let location = sources.parameter(&operation.location, name, place);
            (
                sources.field(&location.address, "description"),
                description(&value)?,
            )
        }
        ("response-schema", "response-schema") => {
            let status = metadata["status"]
                .as_str()
                .filter(|status| status.starts_with('2') && !matches!(*status, "204" | "205"))
                .ok_or_else(|| {
                    AuditError::new("PATCH_TARGET", "Stored success status is invalid")
                })?;
            let media = metadata["mediaType"]
                .as_str()
                .ok_or_else(|| AuditError::new("PATCH_TARGET", "Stored media type is missing"))?;
            let at = sources.follow(&sources.field(
                &sources.field(&operation.location.address, "responses"),
                status,
            ));
            let mut response = sources
                .value(&at)
                .cloned()
                .ok_or_else(|| AuditError::new("PATCH_TARGET", "Response no longer exists"))?;
            let schema = value
                .get("schema")
                .filter(|schema| schema.is_object() || schema.is_boolean())
                .ok_or_else(|| {
                    AuditError::new("AUTHORED_INPUT_REQUIRED", "Provide a JSON response schema")
                })?;
            if response.get("content").is_none() {
                response["content"] = serde_json::json!({});
            }
            if response["content"].get(media).is_none() {
                response["content"][media] = serde_json::json!({});
            }
            response["content"][media]["schema"] = schema.clone();
            (at, response)
        }
        _ => {
            return Err(AuditError::new(
                "INVALID_STORED_FINDING",
                "Stored suggestion does not identify an editable authored field",
            ));
        }
    };
    let location = sources.location(&at);
    let rule = catalogue::find(rule_id)
        .ok_or_else(|| AuditError::new("UNKNOWN_RULE", "Stored rule is no longer supported"))?;
    let finding = Finding {
        key: input.key.clone(),
        fingerprint: crate::rules::hash(input.key.as_bytes()),
        rule_id: rule_id.into(),
        title: rule.title.into(),
        why: rule.why.into(),
        severity: crate::Severity::Medium,
        target: target.into(),
        message: "Reviewed edit of a previously applied authored suggestion".into(),
        fixable: true,
        ignorable: true,
        location,
        related: vec![],
        categories: rule.categories.to_vec(),
        impact: rule.why.into(),
        evidence: "authored-review".into(),
        remediation: rule.remediation.into(),
        code: None,
        fix: serde_json::from_value(metadata.clone()).ok(),
        exclusion_reason: None,
        baseline: false,
    };
    let edit = Edit {
        uri: at.uri,
        pointer: at.pointer,
        before: sources.value(&finding.location.address).cloned(),
        after,
    };
    let result = fixes::commit(&request, report, vec![finding.clone()], vec![edit])?;
    Ok(LegacyFixResult {
        schema: result.revised.input.content.unwrap_or_default(),
        audit: result.report,
        applied_key: input.key,
        applied_finding: finding,
    })
}

fn description(value: &Value) -> Result<Value> {
    value
        .as_str()
        .or_else(|| value.get("description").and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| Value::String(text.into()))
        .ok_or_else(|| AuditError::new("AUTHORED_INPUT_REQUIRED", "Description cannot be blank"))
}
