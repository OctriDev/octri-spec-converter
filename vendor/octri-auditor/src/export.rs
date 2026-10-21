//! Portable reports use escaping at their output boundary, never trusted HTML.

use crate::{AuditError, AuditReport, Baseline, Finding, Result, Severity, baseline};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportDiff {
    pub schema_version: String,
    pub ruleset_version: String,
    pub added: Vec<Finding>,
    pub resolved: Vec<Finding>,
    pub unchanged: Vec<String>,
    pub score_before: f64,
    pub score_after: f64,
    pub score_delta: f64,
    pub explanations: Vec<String>,
}

pub fn compare(before: &AuditReport, after: &AuditReport) -> Result<ReportDiff> {
    if before.schema_version != after.schema_version
        || before.manifest.engine_version != after.manifest.engine_version
        || before.manifest.score_method != after.manifest.score_method
        || before.manifest.ruleset_version != after.manifest.ruleset_version
    {
        return Err(AuditError::new(
            "VERSION_MISMATCH",
            "Compare reports with matching engine, schema, ruleset and scoring versions",
        ));
    }
    if before.manifest.config.profiles != after.manifest.config.profiles
        || before.manifest.config.rules != after.manifest.config.rules
    {
        return Err(AuditError::new(
            "POLICY_MISMATCH",
            "Compare reports with the same profiles and rule policy",
        ));
    }
    let old: BTreeSet<_> = before
        .findings
        .iter()
        .map(|finding| &finding.fingerprint)
        .collect();
    let new: BTreeSet<_> = after
        .findings
        .iter()
        .map(|finding| &finding.fingerprint)
        .collect();
    let explanations = after
        .by_rule
        .iter()
        .filter_map(|rule| {
            let previous = before
                .by_rule
                .iter()
                .find(|previous| previous.rule_id == rule.rule_id)?;
            (previous.deduction != rule.deduction || previous.denominator != rule.denominator).then(
                || {
                    format!(
                        "{}: deduction {} → {}; denominator {} → {}",
                        rule.rule_id,
                        previous.deduction,
                        rule.deduction,
                        previous.denominator,
                        rule.denominator
                    )
                },
            )
        })
        .collect();
    Ok(ReportDiff {
        schema_version: after.schema_version.clone(),
        ruleset_version: after.manifest.ruleset_version.clone(),
        added: after
            .findings
            .iter()
            .filter(|finding| !old.contains(&finding.fingerprint))
            .cloned()
            .collect(),
        resolved: before
            .findings
            .iter()
            .filter(|finding| !new.contains(&finding.fingerprint))
            .cloned()
            .collect(),
        unchanged: old.intersection(&new).map(|key| (*key).clone()).collect(),
        score_before: before.score,
        score_after: after.score,
        score_delta: ((after.score - before.score) * 10.0).round() / 10.0,
        explanations,
    })
}

pub fn html(report: &AuditReport) -> String {
    let mut output = String::from(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'\"><title>Octri Spec Audit</title><style>body{font:16px/1.6 system-ui;max-width:1000px;margin:auto;padding:32px;color:#172b2b;background:#f8faf9}h1,h2{line-height:1.2}article{padding:24px 0;border-bottom:1px solid #a6b4b0}code{overflow-wrap:anywhere}table{border-collapse:collapse;width:100%}td,th{text-align:left;border-bottom:1px solid #b7c7c1;padding:10px}small{color:#405c54}</style><main><h1>Octri Spec Audit</h1>",
    );
    output.push_str(&format!("<p><strong>{}</strong> · {:.1}/10 · engine {} · ruleset {}</p><p>Local static analysis. Executed consumer verification: unknown.</p>", escape(&report.status), report.score, escape(&report.manifest.engine_version), escape(&report.manifest.ruleset_version)));
    output.push_str("<h2>Coverage</h2><ul>");
    for limitation in &report.coverage {
        output.push_str(&format!("<li>{}</li>", escape(limitation)));
    }
    output.push_str("</ul><h2>Findings</h2>");
    for finding in &report.findings {
        output.push_str(&format!("<article><small>{:?} · {} · {}</small><h3>{}</h3><p>{}</p><p><code>{}#{}{} </code></p><p><strong>Impact:</strong> {}</p><p><strong>Action:</strong> {}</p></article>", finding.severity, escape(&finding.rule_id), escape(&finding.target), escape(&finding.title), escape(&finding.message), escape(&finding.location.address.uri), escape(&finding.location.address.pointer), finding.location.line.map_or(String::new(), |line| format!(" (line {line}, column {})", finding.location.column.unwrap_or(1))), escape(&finding.impact), escape(&finding.remediation)));
    }
    output.push_str("<h2>Scoring and check status</h2><table><thead><tr><th>Rule</th><th>Status</th><th>Weight</th><th>Findings / denominator</th><th>Deduction</th></tr></thead><tbody>");
    for rule in &report.by_rule {
        output.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}/{}</td><td>{}</td></tr>",
            escape(&rule.rule_id),
            escape(&rule.status),
            rule.weight,
            rule.findings,
            rule.denominator,
            rule.deduction
        ));
    }
    output.push_str("</tbody></table><h2>Excluded findings</h2>");
    for finding in &report.ignored_findings {
        output.push_str(&format!(
            "<p>{}: {} — {}</p>",
            escape(&finding.rule_id),
            escape(&finding.message),
            escape(finding.exclusion_reason.as_deref().unwrap_or(""))
        ));
    }
    output.push_str("</main></html>\n");
    output
}

pub fn sarif(report: &AuditReport) -> Value {
    let rules: Vec<_> = crate::catalogue::RULES.iter().map(|rule| json!({"id":rule.id,"shortDescription":{"text":rule.title},"fullDescription":{"text":rule.why},"help":{"text":rule.remediation}})).collect();
    let results: Vec<_> = report.findings.iter().chain(&report.ignored_findings).map(|finding| {
        let mut region = json!({});
        if let Some(line) = finding.location.line { region["startLine"] = json!(line); }
        if let Some(column) = finding.location.column { region["startColumn"] = json!(column); }
        let mut result = json!({"ruleId":finding.rule_id,"level":level(finding.severity),"message":{"text":finding.message},"locations":[{"physicalLocation":{"artifactLocation":{"uri":finding.location.address.uri},"region":region},"logicalLocations":[{"fullyQualifiedName":finding.location.address.pointer}]}],"partialFingerprints":{"octri/v1":finding.fingerprint},"baselineState":if finding.baseline {"unchanged"} else {"new"}});
        if let Some(reason) = &finding.exclusion_reason { result["suppressions"] = json!([{"kind":"external","justification":reason}]); }
        result
    }).collect();
    json!({"$schema":"https://json.schemastore.org/sarif-2.1.0.json","version":"2.1.0","runs":[{"tool":{"driver":{"name":"Octri Spec Auditor","version":report.manifest.engine_version,"rules":rules}},"results":results,"invocations":[{"executionSuccessful":report.complete}],"properties":{"rulesetVersion":report.manifest.ruleset_version,"coverage":report.coverage}}]})
}

pub fn annotations(report: &AuditReport) -> String {
    report
        .findings
        .iter()
        .map(|finding| {
            let mut fields = vec![
                format!(
                    "file={}",
                    command_escape(&annotation_path(&finding.location.address.uri), true)
                ),
                format!("title={}", command_escape(&finding.rule_id, true)),
            ];
            if let Some(line) = finding.location.line {
                fields.push(format!("line={line}"));
            }
            if let Some(column) = finding.location.column {
                fields.push(format!("col={column}"));
            }
            format!(
                "::{} {}::{}\n",
                level(finding.severity).replace("note", "notice"),
                fields.join(","),
                command_escape(&finding.message, false)
            )
        })
        .collect()
}

fn level(severity: Severity) -> &'static str {
    match severity {
        Severity::High => "error",
        Severity::Medium => "warning",
        Severity::Advice => "note",
    }
}
fn annotation_path(uri: &str) -> String {
    let Ok(parsed) = url::Url::parse(uri) else {
        return uri.into();
    };
    if parsed.scheme() != "file" {
        return uri.into();
    }
    let path = percent_encoding::percent_decode_str(parsed.path()).decode_utf8_lossy();
    match parsed
        .host_str()
        .filter(|host| !host.is_empty() && *host != "localhost")
    {
        Some(host) => format!("//{host}{path}"),
        None => path.into_owned(),
    }
}
fn command_escape(value: &str, property: bool) -> String {
    let value = value
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A");
    if property {
        value.replace(':', "%3A").replace(',', "%2C")
    } else {
        value
    }
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn baseline_json(report: &AuditReport) -> Baseline {
    baseline(report)
}
