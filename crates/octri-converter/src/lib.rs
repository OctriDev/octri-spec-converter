//! Deterministic specification conversion with explicit fidelity review.
//!
//! All entry points are offline. Callers supply every source byte and mapping.

mod adapters;
mod api;
pub mod artifacts;
mod detect;
mod emit;
mod legacy;
mod numbers;
mod schema;
mod source;
mod trace;
mod types;
mod validation;
pub mod wire;

use octri_parser::{Input, NoIo, Operation, Options, Request, ValidationMode};
use serde_json::{Value, json};
use source::{Sources, hash};
use trace::Trace;
pub use types::*;

pub fn detect(request: &ConversionRequest) -> Result<InputFormat> {
    let mut sources = Sources::new(request)?;
    detect::detect(&mut sources, request.config.input_format)
}

/// Convert from immutable sources. Repeating with the same inputs is identical.
pub fn convert(request: &ConversionRequest) -> Result<ConversionReport> {
    let config = request.config.canonicalized();
    let mut sources = Sources::new(request)?;
    let format = detect::detect(&mut sources, config.input_format)?;
    let uri = sources.root.clone();
    let mut trace = Trace::new(&config, &sources);
    let source_version = if matches!(format, InputFormat::Openapi | InputFormat::Swagger) {
        let source = sources.parse(&uri, false)?;
        source
            .get("openapi")
            .or_else(|| source.get("swagger"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    } else {
        None
    };
    let target = config
        .target_version
        .clone()
        .or_else(|| {
            (format == InputFormat::Openapi)
                .then(|| source_version.clone())
                .flatten()
        })
        .unwrap_or_else(|| "3.1.2".into());
    if !TARGET_VERSIONS.contains(&target.as_str()) {
        return Err(ConversionError::new(
            "TARGET_VERSION",
            format!(
                "Choose a published OpenAPI target: {}",
                TARGET_VERSIONS.join(", ")
            ),
        ));
    }
    let mut document = match format {
        InputFormat::Openapi | InputFormat::Swagger => {
            validate_source(&sources)?;
            // A version migration needs one complete reference graph. A pure
            // format change can retain the caller's original file structure.
            let source = if config.bundle
                || source_version.as_deref() != Some(&target)
                || format == InputFormat::Swagger
            {
                sources.bundle_openapi()?
            } else {
                sources.parse(&uri, false)?
            };
            if format == InputFormat::Swagger && target != "2.0" {
                adapters::swagger::convert(&source, &sources, &config, &mut trace)?
            } else if format == InputFormat::Swagger {
                source
            } else {
                let mut document = source;
                if source_version
                    .as_ref()
                    .is_some_and(|version| version.starts_with("3.0."))
                    && !target.starts_with("3.0.")
                {
                    api::schemas(&mut document, &mut |schema, pointer| {
                        schema::to_modern(
                            schema,
                            &schema::SchemaSite {
                                sources: &sources,
                                uri: &uri,
                                source: pointer,
                                output: pointer,
                            },
                            false,
                            &mut trace,
                        )
                    });
                }
                document
            }
        }
        _ => adapters::convert(format, &mut sources, &config, &mut trace)?,
    };
    trace.validate_credential_selections(&sources, &config);
    if !matches!(format, InputFormat::Openapi | InputFormat::Swagger) {
        schema::modernize_openapi30_imports(&mut document, &sources, &mut trace)?;
    }
    for (name, value) in [("title", &config.title), ("version", &config.api_version)] {
        if let Some(value) = value
            && document["info"][name] != Value::String(value.clone())
        {
            document["info"][name] = json!(value);
            trace.record(
                &sources,
                &uri,
                "",
                &[format!("/info/{name}")],
                "AUTHORED_METADATA",
                Classification::Transformed,
                "Caller supplied explicit API metadata; original remains in source artifacts",
                Some(&json!(value)),
            );
        }
    }
    for (pointer, replacement) in &config.overrides {
        adapters::common::insert_at(&mut document, pointer, replacement.clone())?;
        trace.record(
            &sources,
            &uri,
            "",
            std::slice::from_ref(pointer),
            "AUTHORED_OVERRIDE",
            Classification::Transformed,
            "Caller supplied an explicit replacement; original source remains in the artifact",
            Some(replacement),
        );
    }
    schema::check_dialects(&mut document, &sources, &mut trace);
    if document.get("swagger").is_none() {
        emit::emit(&mut document, &target, &sources, &config, &mut trace)?;
    }
    octri_parser::numbers::canonicalize(&mut document);
    let content = match config.output_format {
        OutputFormat::Json => octri_parser::serialize(&document, &config.limits.parser())?,
        OutputFormat::Yaml => octri_parser::serialize_yaml(&document, &config.limits.parser())?,
    };
    let mut output_uri = format!(
        "memory://converted/openapi.{}",
        if config.output_format == OutputFormat::Json {
            "json"
        } else {
            "yaml"
        }
    );
    while sources.files.contains_key(&output_uri) {
        output_uri.push('_');
    }
    let validation_uri =
        if !config.bundle && matches!(format, InputFormat::Openapi | InputFormat::Swagger) {
            &uri
        } else {
            // Foreign schema resources retain original retrieval identities. The
            // converted API is a new document, so it must have a distinct identity.
            &output_uri
        };
    let (validated, readiness) = validation::analyze(
        &sources,
        &config,
        &document,
        &content,
        validation_uri,
        &target,
    )?;
    for source in &validated.sources {
        if source.uri != *validation_uri {
            sources.remember(source.clone())?;
        }
    }
    let valid = validated.valid;
    let mut checks = vec![Check {
        id: "target-integrity".into(),
        status: if valid { "passed" } else { "failed" }.into(),
        detail: "Reparsed output with strict dialect schema, reference and semantic validation"
            .into(),
    }];
    let audit = if valid && target != "2.0" {
        match readiness.ok_or_else(|| {
            ConversionError::new(
                "AUDIT_STATE",
                "OpenAPI validation returned no readiness outcome",
            )
        })? {
            Ok(report) => {
                if !report.complete {
                    trace.record(&sources, &sources.root, "", &[], "READINESS_AUDIT_INCOMPLETE",
                        Classification::Inferred,
                        "Static readiness audit has unverified or truncated evidence. Inspect its coverage and findings; supply missing evidence or explicitly review this limitation before handoff.",
                        Some(&json!(report)));
                }
                checks.push(Check {
                    id: "readiness-audit".into(),
                    status: if report.complete { "passed" } else { "incomplete" }.into(),
                    detail: "Static readiness evidence is attached; no downstream generator was executed".into(),
                });
                Some(report)
            }
            Err(error) => {
                checks.push(Check {
                    id: "readiness-audit".into(),
                    status: "failed".into(),
                    detail: error,
                });
                None
            }
        }
    } else {
        None
    };
    if target == "2.0" {
        checks.push(Check{id:"readiness-audit".into(),status:"not-applicable".into(),detail:"Swagger 2.0 receives strict parser validation. The Spec Auditor accepts OpenAPI 3.x only; no legacy readiness score is claimed.".into()});
    }
    let status = if !valid || trace.blocked() || checks.iter().any(|check| check.status == "failed")
    {
        Status::Blocked
    } else if trace.needs_review() {
        Status::NeedsReview
    } else {
        Status::Complete
    };
    let handoff_allowed = valid && status == Status::Complete;
    let mut diagnostics = validated.diagnostics;
    if trace.truncated {
        checks.push(Check{id:"fidelity-coverage".into(),status:"failed".into(),detail:"Decision limit reached. Increase the lowered limit or split the source; incomplete fidelity reports cannot complete.".into()});
    }
    // Stable ordering is part of the portable report contract.
    diagnostics.sort_by(|a, b| {
        a.location
            .address
            .cmp(&b.location.address)
            .then(a.code.cmp(&b.code))
    });
    Ok(ConversionReport {
        schema_version: REPORT_VERSION.into(),
        manifest: Manifest {
            engine_version: ENGINE_VERSION.into(),
            parser_version: "0.1.0".into(),
            processing_mode: "local-offline".into(),
            root: uri,
            source_format: format,
            target_version: target,
            files: sources.manifest(),
            config: config.clone(),
        },
        status,
        valid,
        handoff_allowed,
        output: Some(OutputFile {
            uri: output_uri,
            sha256: hash(content.as_bytes()),
            content,
            encoding: config.output_format,
        }),
        document: Some(document),
        inspection: validated.inspection,
        mapping_requests: trace.mapping_requests.values().cloned().collect(),
        duplicate_requests: trace.duplicate_requests.values().cloned().collect(),
        credential_selection_requests: trace.credential_requests.values().cloned().collect(),
        decisions: trace.finish(),
        diagnostics,
        checks,
        audit,
        original: request.clone(),
    })
}

fn validate_source(sources: &Sources) -> Result<()> {
    let options = Options {
        files: sources
            .files
            .iter()
            .filter(|(name, _)| *name != &sources.root)
            .map(|(name, text)| (name.clone(), text.clone()))
            .collect(),
        limits: sources.limits.parser(),
        validate: octri_parser::ValidationOptions {
            mode: ValidationMode::Strict,
            ..Default::default()
        },
        ..Default::default()
    };
    octri_parser::execute(
        Request {
            operation: Operation::Validate,
            input: Input {
                uri: sources.root.clone(),
                content: Some(sources.text(&sources.root)?.into()),
                generated: false,
            },
            options,
        },
        &mut NoIo,
    )?;
    Ok(())
}

pub fn formats() -> Value {
    json!({"schemaVersion":REPORT_VERSION,"engineVersion":ENGINE_VERSION,"targets":TARGET_VERSIONS,"inputs":["openapi","swagger","postman","raml","insomnia","har","curl","asyncapi","graphql","protobuf","wsdl"],"offline":true,"httpMappingsRequiredForNonHttp":true})
}
