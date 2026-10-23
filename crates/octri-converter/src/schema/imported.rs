//! Foreign formats may reference schemas owned by an OpenAPI 3.0 document.
//! Validate that source dialect before migrating nodes with its physical origin.
use crate::{ConversionError, Result, api, source::Sources, trace::Trace};
use octri_parser::{Address, Diagnostic, SchemaDialect, Severity};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::OnceLock,
};

static OPENAPI30: OnceLock<std::result::Result<jsonschema::ValidatorMap, String>> = OnceLock::new();

pub(crate) fn modernize_openapi30_imports(
    document: &mut Value,
    sources: &Sources,
    trace: &mut Trace,
) -> Result<()> {
    if !sources
        .documents
        .values()
        .any(|source| is_openapi30(&source.document))
    {
        return Ok(());
    }
    let mut nodes = BTreeSet::new();
    let mut selection = document.clone();
    api::schemas(&mut selection, &mut |schema, pointer| {
        super::walk(schema, pointer, &mut |schema, at| {
            if let Some(origin) = sources.origins.get(at)
                && sources
                    .documents
                    .get(&origin.uri)
                    .is_some_and(|source| is_openapi30(&source.document))
            {
                // Bundling creates a resource container whose origin is the
                // API document root. Its $id/$defs are generated structure,
                // not an authored 3.0 Schema Object to validate or migrate.
                if origin.pointer.is_empty() && schema.get("openapi").is_none() {
                    return;
                }
                nodes.insert(origin.clone());
                hide_reference_siblings(schema);
            }
        });
    });
    // Legacy reference bundling may inline a selected object and leave only
    // its target's origin in output. The physical graph still records the
    // owning 3.0 reference and therefore its inherited schema semantics.
    for edge in &sources.schema_references {
        if let Some((pointer, _)) = edge.from.address.pointer.rsplit_once('/')
            && sources
                .documents
                .get(&edge.from.address.uri)
                .is_some_and(|source| is_openapi30(&source.document))
        {
            nodes.insert(Address {
                uri: edge.from.address.uri.clone(),
                pointer: pointer.into(),
            });
        }
        if let Some(target) = &edge.target
            && !target.address.pointer.is_empty()
            && sources
                .documents
                .get(&target.address.uri)
                .is_some_and(|source| is_openapi30(&source.document))
        {
            nodes.insert(target.address.clone());
        }
    }
    extend_unowned_references(&mut nodes, sources)?;
    check_conflicting_defaults(&nodes, sources)?;
    validate_roots(&nodes, sources)?;
    let legacy_resources: BTreeSet<_> = nodes.iter().map(|node| node.uri.as_str()).collect();
    api::schemas(document, &mut |schema, pointer| {
        super::walk(schema, pointer, &mut |schema, at| {
            let Some(origin) = sources.origins.get(at).filter(|origin| {
                nodes.contains(*origin)
                    || origin.pointer.is_empty() && legacy_resources.contains(origin.uri.as_str())
            }) else {
                return;
            };
            // Generated resource identities keep rewritten references valid.
            // They are not authored Reference Object siblings to discard.
            let original = sources
                .documents
                .get(&origin.uri)
                .and_then(|source| source.document.pointer(&origin.pointer));
            let generated: Vec<_> = ["$id", "id", "$schema"]
                .into_iter()
                .filter_map(|key| {
                    (original.is_some_and(|value| value.get(key).is_none()))
                        .then(|| schema.get(key).cloned().map(|value| (key, value)))
                        .flatten()
                })
                .collect();
            if nodes.contains(origin) {
                super::modernize_node(
                    schema,
                    at,
                    &super::SchemaSite {
                        sources,
                        uri: &origin.uri,
                        source: &origin.pointer,
                        output: at,
                    },
                    false,
                    trace,
                );
            }
            for (key, value) in generated {
                match key {
                    "id" => {
                        if let Some(map) = schema.as_object_mut() {
                            map.remove("id");
                        }
                        schema["$id"] = value;
                    }
                    "$schema" => {
                        schema["$schema"] = Value::String(SchemaDialect::Draft202012.uri().into())
                    }
                    _ => schema[key] = value,
                }
            }
        });
    });
    let already_recorded: BTreeSet<_> = trace
        .decisions
        .iter()
        .filter(|decision| decision.code == "IGNORED_REFERENCE_SIBLINGS")
        .map(|decision| decision.source.address.clone())
        .collect();
    for node in &nodes {
        if already_recorded.contains(node) {
            continue;
        }
        if let Some(original) = sources
            .documents
            .get(&node.uri)
            .and_then(|source| source.document.pointer(&node.pointer))
            .filter(|value| {
                value.get("$ref").is_some() && value.as_object().is_some_and(|map| map.len() > 1)
            })
        {
            trace.record(sources, &node.uri, &node.pointer, &[], "IGNORED_REFERENCE_SIBLINGS",
                crate::Classification::Omitted,
                "Ignored OpenAPI 3.0 Reference Object siblings remain inactive, including resource identifiers and dialect selectors", Some(original));
        }
    }
    Ok(())
}

fn is_openapi30(document: &Value) -> bool {
    document
        .get("openapi")
        .and_then(Value::as_str)
        .is_some_and(|version| version.starts_with("3.0."))
}

/// Plain external Schema Objects inherit 3.0 semantics from their referring API.
/// A referenced API document or explicit JSON Schema resource owns its dialect.
fn extend_unowned_references(nodes: &mut BTreeSet<Address>, sources: &Sources) -> Result<()> {
    let targets: BTreeMap<_, _> = sources
        .schema_references
        .iter()
        .filter(|edge| edge.kind == "$ref")
        .filter_map(|edge| {
            edge.target
                .as_ref()
                .map(|target| (&edge.from.address, &target.address))
        })
        .collect();
    let mut pending: VecDeque<_> = nodes.iter().cloned().collect();
    let mut work = nodes.len();
    while let Some(origin) = pending.pop_front() {
        let Some(original) = sources
            .documents
            .get(&origin.uri)
            .and_then(|source| source.document.pointer(&origin.pointer))
        else {
            continue;
        };
        if original.get("$ref").and_then(Value::as_str).is_none() {
            continue;
        }
        let reference_site = Address {
            uri: origin.uri.clone(),
            pointer: crate::source::child(&origin.pointer, "$ref"),
        };
        let Some(target) = targets.get(&reference_site).copied() else {
            continue;
        };
        if nodes.contains(target) {
            continue;
        }
        let Some(source) = sources.documents.get(&target.uri) else {
            continue;
        };
        if ["openapi", "swagger", "asyncapi"]
            .iter()
            .any(|key| source.document.get(key).is_some())
        {
            continue;
        }
        let mut parent = target.pointer.as_str();
        let declared = loop {
            if source
                .document
                .pointer(parent)
                .is_some_and(|value| value.get("$schema").is_some())
            {
                break true;
            }
            let Some((prefix, _)) = parent.rsplit_once('/') else {
                break false;
            };
            parent = prefix;
        };
        if declared {
            continue;
        }
        let Some(value) = source.document.pointer(&target.pointer) else {
            continue;
        };
        super::walk_in_dialect(
            &mut value.clone(),
            &target.pointer,
            SchemaDialect::Draft4,
            &mut |_, pointer, _| {
                work = work.saturating_add(1);
                if work <= sources.limits.max_nodes {
                    let address = Address {
                        uri: target.uri.clone(),
                        pointer: pointer.into(),
                    };
                    if nodes.insert(address.clone()) {
                        pending.push_back(address);
                    }
                }
            },
        );
        if work > sources.limits.max_nodes {
            return Err(ConversionError::new(
                "IMPORTED_OPENAPI30_LIMIT",
                "Imported legacy schema traversal exceeds configured node budget",
            ));
        }
    }
    Ok(())
}

/// Remove children only from a temporary traversal model. Explicitly selected
/// descendants still appear as separate bundle roots and retain validation.
fn hide_reference_siblings(schema: &mut Value) {
    if schema.get("$ref").is_some()
        && let Some(map) = schema.as_object_mut()
    {
        map.retain(|key, _| key == "$ref");
    }
}

fn check_conflicting_defaults(nodes: &BTreeSet<Address>, sources: &Sources) -> Result<()> {
    for edge in &sources.schema_references {
        let Some(target) = edge
            .target
            .as_ref()
            .filter(|target| nodes.contains(&target.address))
        else {
            continue;
        };
        if sources
            .documents
            .get(&target.address.uri)
            .is_some_and(|source| source.document.get("openapi").is_some())
        {
            continue;
        }
        let Some((pointer, _)) = edge.from.address.pointer.rsplit_once('/') else {
            continue;
        };
        let origin = Address {
            uri: edge.from.address.uri.clone(),
            pointer: pointer.into(),
        };
        if nodes.contains(&origin) {
            continue;
        }
        return Err(ConversionError {
            code: "SCHEMA_DIALECT_CONFLICT".into(),
            message: "A plain schema is imported with both OpenAPI 3.0 and another schema dialect. Declare its JSON Schema dialect or supply separate explicit payload replacements.".into(),
            diagnostics: vec![Diagnostic {
                code: "SCHEMA_DIALECT_CONFLICT".into(), severity: Severity::Error,
                message: "This reference also selects a schema that requires OpenAPI 3.0 migration".into(),
                location: edge.from.clone(), related: vec![target.clone()],
            }],
        });
    }
    Ok(())
}

/// The parser's pinned OpenAPI meta-schema also validates isolated external
/// Schema Objects. This check never resolves references or loads new sources.
/// The crate carries its own copy so it packages for crates.io;
/// scripts/verify-vendor.mjs keeps it identical to the vendored parser's.
fn validate_schema(value: &Value, at: &Address, sources: &Sources) -> Result<()> {
    let compiled = OPENAPI30.get_or_init(|| {
        let schema: Value = serde_json::from_str(include_str!("../../schemas/openapi-3.0.json"))
            .map_err(|error| error.to_string())?;
        jsonschema::options()
            .offline()
            .build_map(&schema)
            .map_err(|error| error.to_string())
    });
    let validators = compiled
        .as_ref()
        .map_err(|error| ConversionError::new("VALIDATOR_INITIALIZATION", error.clone()))?;
    let path = if value.get("$ref").is_some() {
        "#/definitions/Reference"
    } else {
        "#/definitions/Schema"
    };
    let validator = validators.get(path).ok_or_else(|| {
        ConversionError::new(
            "VALIDATOR_INITIALIZATION",
            "OpenAPI schema validator is missing",
        )
    })?;
    let diagnostics: Vec<_> = validator
        .iter_errors(value)
        .take(sources.limits.parser().max_diagnostics)
        .map(|error| Diagnostic {
            code: "OPENAPI_SCHEMA".into(),
            severity: Severity::Error,
            message: error.to_string(),
            location: sources.physical_location(&Address {
                uri: at.uri.clone(),
                pointer: format!("{}{}", at.pointer, error.instance_path()),
            }),
            related: Vec::new(),
        })
        .collect();
    if diagnostics.is_empty() {
        return Ok(());
    }
    Err(ConversionError {
        code: "IMPORTED_OPENAPI30_INVALID".into(),
        message: "Imported OpenAPI 3.0 schema must validate before migration".into(),
        diagnostics,
    })
}

/// Validate each source tree once. An explicitly selected descendant of an
/// ignored Reference Object sibling needs its own check: the ancestor's
/// Reference Object validator deliberately does not inspect that sibling.
fn validate_roots(nodes: &BTreeSet<Address>, sources: &Sources) -> Result<()> {
    for at in nodes {
        let Some(source) = sources.documents.get(&at.uri) else {
            continue;
        };
        let mut parent = at.pointer.as_str();
        let covered = loop {
            let Some((prefix, _)) = parent.rsplit_once('/') else {
                break false;
            };
            parent = prefix;
            if nodes.contains(&Address {
                uri: at.uri.clone(),
                pointer: parent.into(),
            }) {
                break source
                    .document
                    .pointer(parent)
                    .is_some_and(|value| value.get("$ref").is_none());
            }
        };
        if !covered && let Some(value) = source.document.pointer(&at.pointer) {
            validate_schema(value, at, sources)?;
        }
    }
    Ok(())
}
