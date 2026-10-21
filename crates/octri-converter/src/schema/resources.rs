//! Legacy targets have no schema resources. Localize a complete static graph
//! before removing its resource metadata; never approximate dynamic scope.
use crate::{
    Classification, Config, Result, api,
    source::{Sources, hash},
    trace::Trace,
};
use octri_parser::{
    Input, NoIo, Operation, Options, Request, SchemaDialect, ValidationMode, ValidationOptions,
};
use serde_json::Value;

const FRAGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'/')
    .remove(b'~')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_');

pub(crate) fn prepare_legacy_schemas(
    document: &mut Value,
    sources: &Sources,
    config: &Config,
    trace: &mut Trace,
) -> Result<bool> {
    let declared = document.get("jsonSchemaDialect").and_then(Value::as_str);
    let default = declared
        .and_then(SchemaDialect::from_uri)
        .unwrap_or_default();
    let mut scoped = declared.is_some();
    let mut unsafe_scope = declared.is_some_and(|uri| SchemaDialect::from_uri(uri).is_none());
    api::schemas(document, &mut |schema, pointer| {
        super::walk_in_dialect(schema, pointer, default, &mut |schema, _, dialect| {
            let identifier = if dialect == SchemaDialect::Draft4 {
                "id"
            } else {
                "$id"
            };
            scoped |= schema.get(identifier).is_some()
                || schema.get("$schema").is_some()
                || !dialect.ignores_reference_siblings() && schema.get("$anchor").is_some();
            let dynamic_keywords = if dialect == SchemaDialect::Draft201909 {
                ["$recursiveRef", "$recursiveAnchor"]
            } else {
                ["$dynamicRef", "$dynamicAnchor"]
            };
            unsafe_scope |= schema
                .get("$schema")
                .and_then(Value::as_str)
                .is_some_and(|uri| SchemaDialect::from_uri(uri).is_none())
                || !dialect.ignores_reference_siblings()
                    && dynamic_keywords.iter().any(|key| schema.get(key).is_some());
        });
    });
    let content = octri_parser::serialize(document, &config.limits.parser())?;
    // Keep the temporary retrieval identity distinct from authored IDs.
    // Resolution receives only this document, with no supplied files or I/O.
    let uri = format!("memory://octri-scope/{}.json", hash(content.as_bytes()));
    let report = match octri_parser::execute(
        Request {
            operation: Operation::Analyze,
            input: Input {
                uri: uri.clone(),
                content: Some(content.clone()),
                generated: true,
            },
            options: Options {
                limits: sources.generated_limits(document, &content),
                // Resource metadata must be valid before it can be removed.
                // Resolution alone cannot detect malformed dialect selectors.
                validate: ValidationOptions {
                    mode: ValidationMode::Strict,
                    schema: true,
                    spec: false,
                },
                ..Default::default()
            },
        },
        &mut NoIo,
    ) {
        Ok(report) => report,
        Err(error) => {
            record_invalid(sources, trace, &error.diagnostics);
            return Ok(false);
        }
    };
    if !report.valid {
        record_invalid(sources, trace, &report.diagnostics);
        return Ok(false);
    }
    if unsafe_scope
        || report.references.iter().any(|edge| {
            edge.kind != "$ref"
                || !edge.resolved
                || edge
                    .target
                    .as_ref()
                    .is_none_or(|target| target.address.uri != uri)
        })
    {
        return Ok(false);
    }
    let empty_removed = super::definitions::remove_unused_empty_maps(
        document,
        default,
        &report.references,
        sources,
        trace,
    );
    let definitions = super::definitions::Definitions::plan(document, default, &report.references);
    if report
        .references
        .iter()
        .filter_map(|edge| edge.target.as_ref())
        .any(|target| definitions.removes_container(&target.address.pointer))
    {
        trace.record(sources,&sources.root,"",&[],"SCHEMA_DEFINITION_MAP_REFERENCE",Classification::Blocked,
            "A reference selects an entire reusable-definition map as a schema. Supply an explicit schema replacement before legacy migration; extracting its entries could change its assertion keywords.",None);
        return Ok(false);
    }
    let hoisted = !definitions.is_empty();
    for edge in report.references {
        let Some(target) = edge.target else {
            continue;
        };
        if let Some(reference) = document.pointer_mut(&edge.from.address.pointer) {
            *reference = Value::String(format!(
                "#{}",
                percent_encoding::utf8_percent_encode(
                    &definitions.target(&target.address.pointer),
                    FRAGMENT
                )
            ));
        }
    }
    definitions.apply(document, sources, trace)?;
    Ok(scoped || hoisted || empty_removed)
}

fn record_invalid(sources: &Sources, trace: &mut Trace, diagnostics: &[octri_parser::Diagnostic]) {
    let mut recorded = false;
    for diagnostic in diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == octri_parser::Severity::Error)
    {
        let pointer = &diagnostic.location.address.pointer;
        trace.record(sources, &sources.root, "", &[], "SCHEMA_SOURCE_INVALID", Classification::Blocked,
            format!("Source schema validation failed before legacy migration: {} at pre-migration path {pointer}: {}. Correct the source or authored replacement.", diagnostic.code, diagnostic.message),
            Some(&serde_json::json!(diagnostic)));
        recorded = true;
    }
    if !recorded {
        trace.record(sources, &sources.root, "", &[], "SCHEMA_SOURCE_INVALID", Classification::Blocked,
            "Source schema validation did not complete before legacy migration; target validity cannot establish source meaning.", None);
    }
}
