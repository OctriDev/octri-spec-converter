//! Reviewed, transactional patches. Input bytes remain available for exact undo.

use crate::{AuditError, AuditReport, AuditRequest, Edit, Finding, Result, audit, rules::hash};
use octri_parser::{NoIo, Operation, Options, Request, ValidationMode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FixRequest {
    pub request: AuditRequest,
    pub keys: Vec<String>,
    /// The exact prior manifest prevents applying a preview to edited source.
    pub expected_files: BTreeMap<String, String>,
    #[serde(default)]
    pub inputs: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixResult {
    pub original: AuditRequest,
    pub revised: AuditRequest,
    pub edits: Vec<Edit>,
    pub applied_findings: Vec<Finding>,
    pub report: AuditReport,
    pub format_notice: String,
}

/// Preview and apply share this pure function. A host commits `revised` only
/// after review; restoring `original` implements lossless undo without I/O.
pub fn apply(input: &FixRequest) -> Result<FixResult> {
    if input.keys.is_empty() {
        return Err(AuditError::new(
            "NO_SELECTION",
            "Select at least one proposed fix",
        ));
    }
    let before = audit(&input.request)?;
    let actual: BTreeMap<_, _> = before
        .manifest
        .files
        .iter()
        .map(|file| (file.uri.clone(), file.sha256.clone()))
        .collect();
    if input.expected_files != actual {
        return Err(AuditError::new(
            "STALE_PREVIEW",
            "Source files changed since the patch preview",
        ));
    }
    let mut selected = Vec::new();
    let mut edits = Vec::new();
    let mut addresses = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for key in &input.keys {
        if !keys.insert(key) {
            return Err(AuditError::new(
                "DUPLICATE_SELECTION",
                "Select each finding once",
            ));
        }
        let matches: Vec<_> = before
            .findings
            .iter()
            .filter(|finding| finding.key == *key)
            .collect();
        if matches.len() != 1 {
            return Err(AuditError::new(
                "AMBIGUOUS_SELECTION",
                format!("Finding key must identify exactly one live finding: {key}"),
            ));
        }
        let finding = matches[0];
        let proposal = finding.fix.as_ref().ok_or_else(|| {
            AuditError::new(
                "NO_SAFE_FIX",
                format!("No deterministic or reviewed proposal for {key}"),
            )
        })?;
        for original_edit in &proposal.edits {
            let mut edit = original_edit.clone();
            if proposal.kind == "text" {
                let input = input
                    .inputs
                    .get(key)
                    .and_then(|input| {
                        input
                            .as_str()
                            .or_else(|| input.get("description").and_then(Value::as_str))
                    })
                    .map(str::trim)
                    .filter(|input| !input.is_empty())
                    .ok_or_else(|| {
                        AuditError::new(
                            "AUTHORED_INPUT_REQUIRED",
                            "Write the actual description before previewing this patch",
                        )
                    })?;
                edit.after = Value::String(input.into());
            } else if proposal.kind == "response-schema" {
                let authored_json = input
                    .inputs
                    .get(key)
                    .and_then(|input| input.get("schemaJson"))
                    .and_then(Value::as_str);
                let parsed_schema = if let Some(content) = authored_json {
                    let output = octri_parser::execute_document(
                        octri_parser::Request {
                            operation: octri_parser::Operation::Parse,
                            input: octri_parser::Input {
                                uri: "memory://audit/authored-schema.json".into(),
                                content: Some(format!("{{\"openapi\":\"3.1.2\",\"info\":{{\"title\":\"Authored schema\",\"version\":\"1.0.0\"}},\"paths\":{{}},\"components\":{{\"schemas\":{{\"Authored\":{content}}}}}}}")),
                                generated: false,
                            },
                            options: octri_parser::Options {
                                limits: input.request.config.limits.parser(),
                                ..Default::default()
                            },
                        },
                        &mut crate::NoIo,
                    )
                    .map_err(|error| AuditError::new("INVALID_AUTHORED_SCHEMA", error.message))?;
                    output.document.and_then(|document| {
                        document.pointer("/components/schemas/Authored").cloned()
                    })
                } else {
                    None
                };
                let schema = parsed_schema
                    .as_ref()
                    .or_else(|| input.inputs.get(key).and_then(|input| input.get("schema")))
                    .filter(|schema| schema.is_object() || schema.is_boolean())
                    .ok_or_else(|| {
                        AuditError::new(
                            "AUTHORED_INPUT_REQUIRED",
                            "Provide the actual JSON response schema",
                        )
                    })?;
                let mut response = edit.before.clone().ok_or_else(|| {
                    AuditError::new("PATCH_SOURCE", "The declared response is missing")
                })?;
                let media_type = proposal.media_type.as_deref().ok_or_else(|| {
                    AuditError::new("PATCH_SOURCE", "Missing response media type")
                })?;
                if response.get("content").is_none() {
                    response["content"] = serde_json::json!({});
                }
                if response["content"].get(media_type).is_none() {
                    response["content"][media_type] = serde_json::json!({});
                }
                response["content"][media_type]["schema"] = schema.clone();
                edit.after = response;
            }
            if addresses.iter().any(|(uri, pointer): &(String, String)| {
                uri == &edit.uri
                    && (pointer == &edit.pointer
                        || pointer.starts_with(&format!("{}/", edit.pointer))
                        || edit.pointer.starts_with(&format!("{pointer}/")))
            }) {
                return Err(AuditError::new(
                    "PATCH_CONFLICT",
                    "Selected fixes overlap; apply and re-audit separately",
                ));
            }
            addresses.insert((edit.uri.clone(), edit.pointer.clone()));
            edits.push(edit);
        }
        selected.push(finding.clone());
    }
    commit(&input.request, before, selected, edits)
}

pub(crate) fn commit(
    request: &AuditRequest,
    before: AuditReport,
    selected: Vec<Finding>,
    edits: Vec<Edit>,
) -> Result<FixResult> {
    let supplied_files = crate::canonical_files(request)?;
    let mut options = Options {
        files: supplied_files.clone(),
        ..Options::default()
    };
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
    let mut documents: BTreeMap<_, _> = parsed
        .sources
        .iter()
        .map(|source| (source.uri.clone(), source.document.clone()))
        .collect();
    for edit in &edits {
        let document = documents.get_mut(&edit.uri).ok_or_else(|| {
            AuditError::new("PATCH_SOURCE", "Patch source is not part of this artifact")
        })?;
        if document.pointer(&edit.pointer).cloned() != edit.before {
            return Err(AuditError::new(
                "STALE_PATCH",
                "Patch before-value no longer matches",
            ));
        }
        set(document, &edit.pointer, edit.after.clone())?;
    }
    let mut revised = request.clone();
    revised.input.uri = parsed.registry.root_uri.clone();
    revised.files = supplied_files;
    // Keep unreferenced uploaded files too. Only selected documents are rendered.
    revised.files.extend(
        parsed
            .sources
            .iter()
            .filter(|source| source.uri != revised.input.uri)
            .map(|source| (source.uri.clone(), source.text.clone())),
    );
    for uri in edits.iter().map(|edit| &edit.uri).collect::<BTreeSet<_>>() {
        let document = documents
            .get(uri)
            .ok_or_else(|| AuditError::new("PATCH_SOURCE", "Missing edited document"))?;
        let text = serde_json::to_string_pretty(document)
            .map_err(|error| AuditError::new("PATCH_SERIALIZE", error.to_string()))?
            + "\n";
        if uri == &revised.input.uri {
            revised.input.content = Some(text);
            revised.input.generated = false;
        } else {
            revised.files.insert(uri.clone(), text);
        }
    }
    let mut after = audit(&revised)?;
    let old_integrity: BTreeSet<_> = before
        .findings
        .iter()
        .filter(|finding| !finding.ignorable)
        .map(|finding| (&finding.code, &finding.location.address))
        .collect();
    if after
        .findings
        .iter()
        .filter(|finding| !finding.ignorable)
        .any(|finding| !old_integrity.contains(&(&finding.code, &finding.location.address)))
    {
        return Err(AuditError::new(
            "PATCH_INVALID",
            "Patch introduces an integrity error; no files were changed",
        ));
    }
    if selected.iter().any(|selected| {
        after.findings.iter().any(|finding| {
            finding.rule_id == selected.rule_id
                && finding.target == selected.target
                && finding.location.address == selected.location.address
        })
    }) {
        return Err(AuditError::new(
            "PATCH_INEFFECTIVE",
            "Selected fix did not clear its finding; no files were changed",
        ));
    }
    after.applied_findings = selected.clone();
    Ok(FixResult { original: request.clone(), revised, edits, applied_findings: selected, report: after,
        format_notice: "Edited documents are exported as formatted JSON. Original JSON/YAML bytes and comments remain in original for exact undo and download.".into() })
}

fn set(document: &mut Value, pointer: &str, value: Value) -> Result<()> {
    let (parent, token) = pointer.rsplit_once('/').ok_or_else(|| {
        AuditError::new("PATCH_POINTER", "Patches cannot replace the document root")
    })?;
    let key = token.replace("~1", "/").replace("~0", "~");
    let parent = document
        .pointer_mut(parent)
        .ok_or_else(|| AuditError::new("PATCH_POINTER", "Patch parent does not exist"))?;
    let map = parent
        .as_object_mut()
        .ok_or_else(|| AuditError::new("PATCH_POINTER", "Patch parent must be an object"))?;
    map.insert(key, value);
    Ok(())
}

pub fn hashes(report: &AuditReport) -> BTreeMap<String, String> {
    report
        .manifest
        .files
        .iter()
        .map(|file| (file.uri.clone(), file.sha256.clone()))
        .collect()
}

pub fn revision(request: &AuditRequest) -> String {
    hash(request.input.content.as_deref().unwrap_or("").as_bytes())
}
