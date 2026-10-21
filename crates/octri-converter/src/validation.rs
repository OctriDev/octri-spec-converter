//! One strict output analysis supplies integrity, inventory and readiness.
use crate::{Config, OutputFormat, Result, source::Sources};
use octri_parser::{Input, NoIo, Operation, Options, Request, ValidationMode};
use serde_json::Value;
use std::collections::BTreeMap;

type Readiness = Option<std::result::Result<octri_auditor::AuditReport, String>>;

pub(crate) fn analyze(
    sources: &Sources,
    config: &Config,
    document: &Value,
    content: &str,
    uri: &str,
    target: &str,
) -> Result<(octri_parser::Report, Readiness)> {
    let input = Input {
        uri: uri.into(),
        content: Some(content.into()),
        generated: false,
    };
    let files: BTreeMap<_, _> = if config.bundle {
        // A bundled artifact must stand on its own. Original files remain in
        // conversion provenance, but cannot mask missing output references.
        BTreeMap::new()
    } else {
        sources
            .files
            .iter()
            .filter(|(name, _)| name.as_str() != uri)
            .map(|(name, text)| (name.clone(), text.clone()))
            .collect()
    };
    let root_format = match config.output_format {
        OutputFormat::Json => octri_parser::SourceFormat::Json,
        OutputFormat::Yaml => octri_parser::SourceFormat::Yaml,
    };
    let limits = sources.generated_limits(document, content);
    let readiness = if target == "2.0" {
        None
    } else {
        let request = octri_auditor::AuditRequest {
            input: input.clone(),
            files: files.clone(),
            config: octri_auditor::Config {
                root_format,
                limits: octri_auditor::AuditLimits {
                    max_document_bytes: limits.max_document_bytes,
                    max_total_bytes: limits.max_total_bytes,
                    max_documents: files.len() + 1,
                    max_nodes: limits.max_nodes.min(1_000_000),
                    max_depth: limits.max_depth,
                },
                ..Default::default()
            },
        };
        match octri_auditor::audit_with_analysis(&request) {
            Ok(result) => return Ok((result.analysis, Some(Ok(result.audit)))),
            Err(error) => Some(Err(error.message)),
        }
    };
    // Swagger has no readiness audit. If auditing fails its own limits, still
    // return strict integrity evidence and expose the mandatory audit failure.
    let parsed = octri_parser::execute(
        Request {
            operation: Operation::Analyze,
            input,
            options: Options {
                files,
                root_format,
                limits,
                validate: octri_parser::ValidationOptions {
                    mode: ValidationMode::Strict,
                    ..Default::default()
                },
                ..Default::default()
            },
        },
        &mut NoIo,
    )?;
    Ok((parsed, readiness))
}
