//! Octri Spec Auditor: one offline engine for native bindings, browsers and CI.
//!
//! The parser owns syntax, references and effective API semantics. This crate
//! owns versioned readiness rules, policy, reviewed patches and portable reports.

pub mod catalogue;
mod examples;
pub mod export;
pub mod fixes;
pub mod legacy;
mod naming;
pub mod policy;
mod rules;
mod source;
mod types;
pub mod wire;

pub use octri_parser::{Input, Location};
pub use types::*;

use octri_parser::{NoIo, Operation, Options, Request, ValidationMode};
use std::collections::{BTreeMap, BTreeSet};

/// Audit text and explicitly supplied virtual files. No filesystem or HTTP
/// capability is created from a URI or a reference embedded in customer input.
pub fn audit(request: &AuditRequest) -> Result<AuditReport> {
    audit_with_analysis(request).map(|result| result.audit)
}

/// Audit and retain the strict parser analysis produced by this call.
/// Consumers such as the converter can inspect that same validated document
/// without parsing it twice. This API never accepts caller-supplied evidence.
#[derive(Debug)]
pub struct AuditedSource {
    pub audit: AuditReport,
    pub analysis: octri_parser::Report,
}

pub fn audit_with_analysis(request: &AuditRequest) -> Result<AuditedSource> {
    policy::validate(&request.config)?;
    if request.input.content.is_none() {
        return Err(AuditError::new(
            "INPUT_REQUIRED",
            "Supply source text; the auditor does not read paths or URLs",
        ));
    }
    let supplied_files = canonical_files(request)?;
    let limits = request.config.limits.parser();
    let mut options = Options {
        files: supplied_files.clone(),
        root_format: request.config.root_format,
        limits,
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
    .map_err(|error| AuditError {
        code: "PARSER_ERROR".into(),
        message: error.message,
        diagnostics: error.diagnostics,
    })?;
    let mut raw = rules::run(&parsed, request.config.max_findings);
    let supported = matches!(
        parsed.dialect,
        Some(
            octri_parser::Dialect::OpenApi30
                | octri_parser::Dialect::OpenApi31
                | octri_parser::Dialect::OpenApi32
        )
    );
    let swagger = parsed.dialect == Some(octri_parser::Dialect::Swagger2)
        || parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "UNSUPPORTED_SWAGGER_VERSION")
        || parsed.sources.iter().any(|source| {
            source.uri == parsed.registry.root_uri
                && source.document.get("openapi").is_none()
                && (source.document.get("swagger").is_some()
                    || source.document.get("swaggerVersion").is_some())
        });
    if swagger {
        raw.push("spec-integrity", "swagger-conversion-required".into(), octri_parser::Address { uri: request.input.uri.clone(), pointer: String::new() }.into(), "Swagger input requires conversion. Use Spec Converter to convert it to OpenAPI, review the converted document, then run the audit.");
        if let Some(finding) = raw
            .values
            .last_mut()
            .filter(|finding| finding.target == "swagger-conversion-required")
        {
            finding.code = Some("SWAGGER_CONVERSION_REQUIRED".into());
            finding.remediation = "Use Spec Converter to convert Swagger to OpenAPI, then audit the converted document. The auditor does not convert or relabel your source.".into();
        }
    }
    let valid = parsed.valid && supported;
    let incomplete_examples = raw
        .values
        .iter()
        .any(|finding| finding.rule_id == "schema-example-unverified");
    let complete = valid && !raw.truncated && !incomplete_examples;
    let mut findings = Vec::new();
    let mut ignored_findings = Vec::new();
    for mut finding in raw.values {
        if let Some(setting) = request.config.rules.get(&finding.rule_id)
            && let Some(severity) = setting.severity
        {
            finding.severity = severity;
        }
        finding.baseline = policy::baseline(&finding, &request.config);
        let disabled = request
            .config
            .rules
            .get(&finding.rule_id)
            .is_some_and(|setting| setting.enabled == Some(false));
        let selected = rules::categories_selected(&finding.categories, &request.config.profiles);
        let reason = if disabled {
            Some("Disabled by explicit rule policy".into())
        } else if !selected {
            Some("Outside selected compatibility profiles".into())
        } else {
            policy::exclusion_reason(&finding, &request.config)
        };
        if let Some(reason) = reason {
            finding.exclusion_reason = Some(reason);
            ignored_findings.push(finding);
        } else {
            findings.push(finding);
        }
    }
    // Stable order is part of the report contract, including across adapters.
    findings.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then(left.rule_id.cmp(&right.rule_id))
            .then(left.fingerprint.cmp(&right.fingerprint))
    });
    ignored_findings.sort_by(|left, right| left.fingerprint.cmp(&right.fingerprint));
    let inspection = parsed.inspection.as_ref();
    let surface = Surface {
        operations: inspection.map_or(0, |inventory| inventory.statistics.operations),
        models: inspection.map_or(0, |inventory| inventory.statistics.schemas),
        paths: inspection.map_or(0, |inventory| inventory.statistics.paths),
        schemas: source::Sources(&parsed).schemas().len(),
    };
    let mut deducted = 0.0;
    let by_rule: Vec<_> = catalogue::RULES.iter().map(|rule| {
        let count = findings.iter().filter(|finding| finding.rule_id == rule.id).count();
        let denominator = match rule.surface { "document" => 1, "paths" => surface.paths.max(1), "schemas" => surface.schemas.max(1), _ => surface.operations.max(1) };
        let cost = if rule.id == "spec-integrity" { if !valid { 10.0 } else { 0.0 } }
            else if count == 0 { 0.0 } else { rule.weight * (0.2 + 0.8 * (count as f64 / denominator as f64).min(1.0)) };
        deducted += cost;
        let disabled = request.config.rules.get(rule.id).is_some_and(|setting| setting.enabled == Some(false));
        let unselected = !rules::categories_selected(rule.categories, &request.config.profiles);
        let (status, reason) = if disabled || unselected { ("skipped", Some(if disabled { "Disabled by explicit rule policy" } else { "Outside selected compatibility profiles" }.into())) }
            else if !supported { ("unsupported", Some("Unsupported or undetected dialect".into())) }
            else if rule.id=="schema-example-valid"&&incomplete_examples{("unsupported",Some("At least one authored example could not be evaluated; see schema-example-unverified".into()))}
            else if inspection.is_none() && rule.id != "spec-integrity" { ("skipped", Some("Parsing failed before an effective inventory was available".into())) }
            else if count > 0 || rule.id == "spec-integrity" && !valid { ("failed", None) }
            else if ignored_findings.iter().any(|finding| finding.rule_id == rule.id) { ("skipped", Some("All findings excluded by policy; review ignoredFindings".into())) }
            else { ("checked", None) };
        RuleResult { rule_id: rule.id.into(), title: rule.title.into(), weight: if rule.id == "spec-integrity" && !valid { 10.0 } else { rule.weight }, findings: count, deduction: round(cost, 2), denominator, status: status.into(), reason }
    }).collect();
    let score = if complete {
        round((10.0 - deducted).max(0.0), 1)
    } else {
        0.0
    };
    let views = [
        Category::Integrity,
        Category::Docs,
        Category::Sdk,
        Category::Mcp,
    ]
    .into_iter()
    .map(|category| {
        let applicable: Vec<_> = by_rule
            .iter()
            .filter(|result| {
                catalogue::find(&result.rule_id)
                    .is_some_and(|rule| rule.categories.contains(&category))
            })
            .collect();
        let selected: Vec<_> = findings
            .iter()
            .filter(|finding| finding.categories.contains(&category))
            .collect();
        let blockers = selected
            .iter()
            .filter(|finding| finding.severity == Severity::High)
            .count();
        let skipped = applicable
            .iter()
            .filter(|result| result.status == "skipped")
            .count();
        let unsupported = applicable
            .iter()
            .filter(|result| result.status == "unsupported")
            .count();
        let checked = applicable
            .iter()
            .filter(|result| result.status == "checked")
            .count();
        let failed = applicable
            .iter()
            .filter(|result| result.status == "failed")
            .count();
        let status = if !valid || !complete || blockers > 0 {
            "blocked"
        } else if skipped > 0 || unsupported > 0 || !selected.is_empty() {
            "needs-review"
        } else {
            "checked"
        };
        let deduction = applicable
            .iter()
            .map(|result| result.deduction)
            .sum::<f64>();
        ReadinessView {
            category,
            status: status.into(),
            score: if valid && complete {
                Some(round((10.0 - deduction).max(0.0), 1))
            } else {
                None
            },
            blockers,
            checked,
            failed,
            skipped,
            unsupported,
            findings: selected
                .iter()
                .map(|finding| finding.fingerprint.clone())
                .collect(),
        }
    })
    .collect();
    let policy_passed = complete
        && !findings.iter().any(|finding| {
            finding.severity >= request.config.fail_on
                && (!request.config.fail_on_new || !finding.baseline)
        });
    let mut coverage = vec![
        "All readiness findings are static analysis. No docs, SDK or MCP generator was executed.".into(),
        "Security checks describe specification declarations; they do not certify runtime API security.".into(),
        "Custom JSON Schema dialect vocabularies and dynamic references require target-specific verification.".into(),
        "Authored examples use an offline JSON Schema resource registry. Unsupported custom vocabularies are reported individually.".into(),
        "SDK naming profile checks case/separator folding and namespace shortening; full generator allocation requires executed verification.".into(),
    ];
    if raw.truncated {
        coverage.push(
            "Finding limit reached: report is incomplete, score is blocked and CI fails.".into(),
        );
    }
    let mut files: Vec<_> = parsed
        .sources
        .iter()
        .map(|source| ArtifactFile {
            uri: source.uri.clone(),
            sha256: rules::hash(source.text.as_bytes()),
            bytes: source.bytes,
        })
        .collect();
    // Invalid syntax can fail before the parser registers a source document.
    if files.is_empty()
        && let Some(content) = &request.input.content
    {
        files.push(ArtifactFile {
            uri: request.input.uri.clone(),
            sha256: rules::hash(content.as_bytes()),
            bytes: content.len(),
        });
    }
    for (uri, content) in supplied_files {
        if !files.iter().any(|file| file.uri == uri) {
            files.push(ArtifactFile {
                uri,
                sha256: rules::hash(content.as_bytes()),
                bytes: content.len(),
            });
        }
    }
    files.sort_by(|left, right| left.uri.cmp(&right.uri));
    let profiles = expanded_profiles(&request.config.profiles).into_iter().map(|id| ProfileResult {
        id, version: "1.0.0".into(), evidence: "static-analysis".into(), executed_verification: "unknown".into(),
        limitations: vec!["Compatibility depends on the actual consumer and its version. No universal generator conformance is claimed.".into()],
    }).collect();
    let audit = AuditReport {
        schema_version: REPORT_VERSION.into(),
        manifest: Manifest {
            engine_version: ENGINE_VERSION.into(),
            ruleset_version: RULESET_VERSION.into(),
            score_method: "octri-10-point/1".into(),
            processing_mode: "local-offline".into(),
            root: request.input.uri.clone(),
            files,
            config: request.config.clone(),
            profiles,
        },
        valid,
        complete,
        status: if !complete {
            "blocked"
        } else if findings.is_empty() {
            "checked"
        } else {
            "needs-review"
        }
        .into(),
        dialect: parsed.dialect,
        score,
        max_score: 10.0,
        findings,
        ignored_findings,
        applied_findings: Vec::new(),
        by_rule,
        surface,
        diagnostics: parsed.diagnostics.clone(),
        views,
        policy_passed,
        coverage,
    };
    Ok(AuditedSource {
        audit,
        analysis: parsed,
    })
}

pub(crate) fn canonical_files(request: &AuditRequest) -> Result<BTreeMap<String, String>> {
    let limits = &request.config.limits;
    if request.files.len() >= limits.max_documents {
        return Err(AuditError::new(
            "LIMIT_FILES",
            "Input exceeds the configured number of files",
        ));
    }
    let root = url::Url::parse(&request.input.uri).map_err(|_| {
        AuditError::new(
            "INPUT_URI",
            "Supply an absolute source URI such as memory://audit/openapi.yaml",
        )
    })?;
    if root.cannot_be_a_base() || root.fragment().is_some() {
        return Err(AuditError::new(
            "INPUT_URI",
            "Root URI must support relative references and have no fragment",
        ));
    }
    let mut total = request.input.content.as_ref().map_or(0, String::len);
    if total > limits.max_document_bytes {
        return Err(AuditError::new(
            "LIMIT_FILE_BYTES",
            "Root source exceeds the configured per-file limit",
        ));
    }
    let mut files = BTreeMap::new();
    for (name, content) in &request.files {
        if content.len() > limits.max_document_bytes {
            return Err(AuditError::new(
                "LIMIT_FILE_BYTES",
                format!("{name} exceeds the configured per-file limit"),
            ));
        }
        total += content.len();
        let uri = root.join(name).map_err(|_| {
            AuditError::new("INPUT_URI", format!("Invalid virtual source name: {name}"))
        })?;
        if uri.fragment().is_some() || uri == root {
            return Err(AuditError::new(
                "DUPLICATE_SOURCE",
                "Virtual files cannot replace the root or contain fragments",
            ));
        }
        if files.insert(uri.to_string(), content.clone()).is_some() {
            return Err(AuditError::new(
                "DUPLICATE_SOURCE",
                "Virtual file names resolve to the same source URI",
            ));
        }
    }
    if total > limits.max_total_bytes {
        return Err(AuditError::new(
            "LIMIT_TOTAL_BYTES",
            "Expanded input exceeds the configured total byte limit",
        ));
    }
    Ok(files)
}

fn expanded_profiles(profiles: &[String]) -> BTreeSet<String> {
    profiles
        .iter()
        .flat_map(|profile| {
            if profile == "sdk" {
                policy::SDK_LANGUAGES
                    .iter()
                    .map(|language| format!("sdk-{language}"))
                    .collect()
            } else {
                vec![profile.clone()]
            }
        })
        .collect()
}

fn round(value: f64, decimals: i32) -> f64 {
    let factor = 10_f64.powi(decimals);
    (value * factor).round() / factor
}

pub fn baseline(report: &AuditReport) -> Baseline {
    Baseline {
        schema_version: report.schema_version.clone(),
        ruleset_version: report.manifest.ruleset_version.clone(),
        fingerprints: report
            .findings
            .iter()
            .filter(|finding| finding.ignorable)
            .map(|finding| finding.fingerprint.clone())
            .collect(),
    }
}

/// Produce a portable, reference-preserving artifact for explicit Octri import.
/// Bundling never changes the dialect or silently enables external retrieval.
pub fn bundle(request: &AuditRequest) -> Result<AuditRequest> {
    let before = audit(request)?;
    if !before.complete {
        return Err(AuditError::new(
            "INTEGRITY_BLOCKED",
            "Correct integrity errors before creating an import artifact",
        ));
    }
    let mut options = Options {
        files: canonical_files(request)?,
        root_format: request.config.root_format,
        limits: request.config.limits.parser(),
        ..Options::default()
    };
    options.validate.mode = ValidationMode::Strict;
    let parsed = octri_parser::execute(
        Request {
            operation: Operation::Bundle,
            input: request.input.clone(),
            options,
        },
        &mut NoIo,
    )
    .map_err(|error| AuditError::new("BUNDLE_FAILED", error.message))?;
    let document = parsed
        .document
        .ok_or_else(|| AuditError::new("BUNDLE_FAILED", "No complete bundle was produced"))?;
    let content = serde_json::to_string_pretty(&document)
        .map_err(|error| AuditError::new("SERIALIZATION", error.to_string()))?
        + "\n";
    let revised = AuditRequest {
        input: Input {
            uri: request.input.uri.clone(),
            content: Some(content),
            generated: false,
        },
        files: BTreeMap::new(),
        config: Config {
            root_format: octri_parser::SourceFormat::Json,
            ..request.config.clone()
        },
    };
    if !audit(&revised)?.valid {
        return Err(AuditError::new(
            "BUNDLE_INVALID",
            "Portable bundle failed integrity revalidation",
        ));
    }
    Ok(revised)
}
