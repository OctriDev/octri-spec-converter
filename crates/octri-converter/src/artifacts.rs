//! Portable reports are reproducible artifacts, never trusted completion flags.
use crate::source::{child, hash};
use crate::{ConversionError, ConversionReport, ConversionRequest, Result};
use serde_json::{Value, json};

pub fn verify(report: &ConversionReport) -> Result<()> {
    if report.manifest.engine_version != crate::ENGINE_VERSION {
        return Err(ConversionError::new(
            "REPORT_VERSION",
            "Reconvert original sources with this engine version before importing the report",
        ));
    }
    let actual = crate::convert(&report.original)?;
    let expected = serde_json::to_value(report)
        .map_err(|error| ConversionError::new("REPORT_ENCODING", error.to_string()))?;
    let actual = serde_json::to_value(actual)
        .map_err(|error| ConversionError::new("REPORT_ENCODING", error.to_string()))?;
    if !octri_parser::numbers::equivalent(&expected, &actual) {
        return Err(ConversionError::new(
            "REPORT_MISMATCH",
            "Report does not match a fresh conversion of its recorded sources and settings",
        ));
    }
    Ok(())
}

pub fn bundle(request: &ConversionRequest) -> Result<ConversionReport> {
    let mut request = request.clone();
    request.config.bundle = true;
    crate::convert(&request)
}

pub fn export(report: &ConversionReport, format: &str) -> Result<Value> {
    verify(report)?;
    match format {
        "json" => serde_json::to_value(report)
            .map_err(|error| ConversionError::new("REPORT_ENCODING", error.to_string())),
        "html" => Ok(json!(html(report))),
        "manifest" => Ok(mapping_manifest(report)),
        "artifact" => {
            let mut files = vec![
                json!({"path":"report.json","content":serde_json::to_string_pretty(report).map_err(|error|ConversionError::new("REPORT_ENCODING",error.to_string()))?}),
                json!({"path":"manifest.json","content":serde_json::to_string_pretty(&mapping_manifest(report)).map_err(|error|ConversionError::new("REPORT_ENCODING",error.to_string()))?}),
            ];
            let mut originals = report.original.files.clone();
            if let Some(content) = &report.original.input.content {
                originals.insert(report.original.input.uri.clone(), content.clone());
            }
            for (index, (uri, content)) in originals.into_iter().enumerate() {
                files.push(json!({"path":format!("sources/{index:03}-{}.txt",&hash(uri.as_bytes())[..12]),"sourceUri":uri,"sha256":hash(content.as_bytes()),"content":content}));
            }
            if let Some(output) = &report.output {
                files.push(json!({"path":if output.encoding==crate::OutputFormat::Json{"openapi.json"}else{"openapi.yaml"},"sha256":output.sha256,"content":output.content}));
            }
            Ok(
                json!({"schemaVersion":crate::REPORT_VERSION,"handoffAllowed":report.handoff_allowed,"files":files}),
            )
        }
        _ => Err(ConversionError::new(
            "EXPORT_FORMAT",
            "Choose json, html, manifest or artifact",
        )),
    }
}

fn mapping_manifest(report: &ConversionReport) -> Value {
    let mut manifest = json!(report.manifest);
    manifest["schemaVersion"] = json!(report.schema_version);
    manifest["status"] = json!(report.status);
    manifest["outputSha256"] = json!(report.output.as_ref().map(|output| &output.sha256));
    manifest["mappings"] = json!(
        report
            .decisions
            .iter()
            .map(|decision| json!({
                "id": decision.id,
                "classification": decision.classification,
                "code": decision.code,
                "source": decision.source,
                "outputPointers": decision.output_pointers,
                "reviewRequired": decision.review_required,
                "reviewed": decision.reviewed,
            }))
            .collect::<Vec<_>>()
    );
    manifest
}

pub fn compare(before: &ConversionReport, after: &ConversionReport) -> Result<Value> {
    verify(before)?;
    verify(after)?;
    let mut changes = Vec::new();
    let mut truncated = false;
    diff(
        before.document.as_ref(),
        after.document.as_ref(),
        "",
        &mut changes,
        &mut truncated,
    );
    Ok(
        json!({"schemaVersion":crate::REPORT_VERSION,"beforeStatus":before.status,"afterStatus":after.status,"changes":changes,"truncated":truncated,"beforeOutputHash":before.output.as_ref().map(|output|&output.sha256),"afterOutputHash":after.output.as_ref().map(|output|&output.sha256)}),
    )
}

fn diff(
    before: Option<&Value>,
    after: Option<&Value>,
    pointer: &str,
    changes: &mut Vec<Value>,
    truncated: &mut bool,
) {
    if match (before, after) {
        (Some(before), Some(after)) => octri_parser::numbers::equivalent(before, after),
        (None, None) => true,
        _ => false,
    } {
        return;
    }
    if changes.len() >= 10_000 {
        *truncated = true;
        return;
    }
    match (before,after){
        (Some(Value::Object(left)),Some(Value::Object(right)))=>{
            let keys=left.keys().chain(right.keys()).collect::<std::collections::BTreeSet<_>>();
            for key in keys{diff(left.get(key),right.get(key),&child(pointer,key),changes,truncated);if *truncated{return;}}
        },
        (Some(Value::Array(left)),Some(Value::Array(right)))=>{
            for index in 0..left.len().max(right.len()){diff(left.get(index),right.get(index),&child(pointer,&index.to_string()),changes,truncated);if *truncated{return;}}
        },
        _=>changes.push(json!({"pointer":pointer,"before":before,"after":after,"kind":if before.is_none(){"added"}else if after.is_none(){"removed"}else{"changed"}})),
    }
}

fn html(report: &ConversionReport) -> String {
    let mut output = String::from(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'\"><title>Specification conversion report</title><style>body{font:16px/1.6 system-ui;max-width:78rem;margin:2rem auto;padding:0 1rem;color:#182121;background:#fafcfb}table{border-collapse:collapse;width:100%}th,td{text-align:left;vertical-align:top;border-bottom:1px solid #cbd5d1;padding:.7rem}code,pre{overflow-wrap:anywhere;white-space:pre-wrap}h1{line-height:1.2}caption{text-align:left;font-weight:600;margin:1rem 0}</style><main><h1>Specification conversion report</h1>",
    );
    output.push_str(&format!("<p>Status: <strong>{:?}</strong>. Strict target validation: {}. Handoff allowed: {}.</p><p>Source: <code>{}</code>. Target: <code>{}</code>.</p>",report.status,report.valid,report.handoff_allowed,escape(&report.manifest.root),escape(&report.manifest.target_version)));
    output.push_str("<table><caption>Fidelity and review decisions</caption><thead><tr><th scope=\"col\">Classification</th><th scope=\"col\">Source</th><th scope=\"col\">Decision</th><th scope=\"col\">Review</th></tr></thead><tbody>");
    for decision in &report.decisions {
        output.push_str(&format!("<tr><td>{:?}</td><td><code>{}#{}</code></td><td><code>{}</code><br>{}</td><td>{}</td></tr>",decision.classification,escape(&decision.source.address.uri),escape(&decision.source.address.pointer),escape(&decision.code),escape(&decision.message),if decision.reviewed{"Acknowledged"}else if decision.review_required{"Required"}else{"Not required"}));
    }
    output.push_str("</tbody></table><h2>Validation diagnostics</h2><ul>");
    for diagnostic in &report.diagnostics {
        output.push_str(&format!(
            "<li><code>{}</code>: {}</li>",
            escape(&diagnostic.code),
            escape(&diagnostic.message)
        ));
    }
    output.push_str("</ul><h2>Source manifest</h2><ul>");
    for file in &report.manifest.files {
        output.push_str(&format!(
            "<li><code>{}</code> — {} bytes, SHA-256 <code>{}</code></li>",
            escape(&file.uri),
            file.bytes,
            escape(&file.sha256)
        ));
    }
    output.push_str("</ul><p>Original sources may contain credentials or personal data. Review artifact contents before sharing.</p></main></html>");
    output
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
