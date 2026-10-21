//! Schema transforms only visit schema positions. Examples and defaults are data.

use crate::source::{Sources, child};
use crate::trace::Trace;
use crate::{Classification, Config};
use serde_json::{Value, json};

mod definitions;
mod imported;
mod resources;
pub(crate) use imported::modernize_openapi30_imports;
pub(crate) use resources::prepare_legacy_schemas;

const SCHEMA_MAPS: &[&str] = &[
    "properties",
    "patternProperties",
    "dependentSchemas",
    "$defs",
    "definitions",
];
const SCHEMA_ARRAYS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];
const SCHEMA_VALUES: &[&str] = &[
    "items",
    "additionalProperties",
    "additionalItems",
    "not",
    "contains",
    "if",
    "then",
    "else",
    "unevaluatedProperties",
    "unevaluatedItems",
    "propertyNames",
    "contentSchema",
];

pub(crate) struct SchemaSite<'a> {
    pub sources: &'a Sources,
    pub uri: &'a str,
    pub source: &'a str,
    pub output: &'a str,
}

impl<'a> SchemaSite<'a> {
    fn at(sources: &'a Sources, uri: &'a str, pointer: &'a str) -> Self {
        Self {
            sources,
            uri,
            source: pointer,
            output: pointer,
        }
    }
    fn source_at(&self, output: &str) -> String {
        format!(
            "{}{}",
            self.source,
            output.strip_prefix(self.output).unwrap_or("")
        )
    }
}

/// Visit every subschema without entering literal instance data.
pub(crate) fn walk(value: &mut Value, pointer: &str, visitor: &mut impl FnMut(&mut Value, &str)) {
    walk_with_default(
        value,
        pointer,
        octri_parser::SchemaDialect::default(),
        visitor,
    );
}

/// Resource keywords in schema positions require preserving an independent base.
/// Unlike transformation visitors, this also observes legacy reference nodes.
pub(crate) fn has_resource_scope(value: &mut Value) -> bool {
    let mut scoped = false;
    walk_in_dialect(value, "", Default::default(), &mut |schema, _, dialect| {
        let identifier = if dialect == octri_parser::SchemaDialect::Draft4 {
            "id"
        } else {
            "$id"
        };
        scoped |= ["$schema", identifier, "$ref"]
            .iter()
            .any(|key| schema.get(key).is_some());
        if !dialect.ignores_reference_siblings() {
            let references = if dialect == octri_parser::SchemaDialect::Draft201909 {
                ["$recursiveRef", "$anchor", "$recursiveAnchor"]
            } else {
                ["$dynamicRef", "$anchor", "$dynamicAnchor"]
            };
            scoped |= references.iter().any(|key| schema.get(key).is_some());
        }
    });
    scoped
}

/// API-level defaults apply until an individual schema declares its own dialect.
pub(crate) fn walk_with_default(
    value: &mut Value,
    pointer: &str,
    inherited: octri_parser::SchemaDialect,
    visitor: &mut impl FnMut(&mut Value, &str),
) {
    walk_in_dialect(
        value,
        pointer,
        inherited,
        &mut |schema, pointer, dialect| {
            if !dialect.ignores_reference_siblings() || schema.get("$ref").is_none() {
                visitor(schema, pointer);
            }
        },
    );
}

fn walk_in_dialect(
    value: &mut Value,
    pointer: &str,
    inherited: octri_parser::SchemaDialect,
    visitor: &mut impl FnMut(&mut Value, &str, octri_parser::SchemaDialect),
) {
    let dialect = value
        .get("$schema")
        .and_then(Value::as_str)
        .and_then(octri_parser::SchemaDialect::from_uri)
        .unwrap_or(inherited);
    let custom = value
        .get("$schema")
        .and_then(Value::as_str)
        .is_some_and(|uri| octri_parser::SchemaDialect::from_uri(uri).is_none());
    let legacy = dialect.ignores_reference_siblings();
    let draft4 = dialect == octri_parser::SchemaDialect::Draft4;
    let tuple_dialect = legacy || dialect == octri_parser::SchemaDialect::Draft201909;
    let reference = value.get("$ref").is_some();
    let tuple_items = value.get("items").is_some_and(Value::is_array);
    visitor(value, pointer, dialect);
    if custom || legacy && reference {
        return;
    }
    if let Some(object) = value.as_object_mut() {
        for key in SCHEMA_MAPS {
            if legacy && matches!(*key, "$defs" | "dependentSchemas") {
                continue;
            }
            if let Some(map) = object.get_mut(*key).and_then(Value::as_object_mut) {
                for (name, schema) in map {
                    walk_in_dialect(schema, &child(&child(pointer, key), name), dialect, visitor);
                }
            }
        }
        for key in SCHEMA_ARRAYS {
            if tuple_dialect && *key == "prefixItems" {
                continue;
            }
            if let Some(items) = object.get_mut(*key).and_then(Value::as_array_mut) {
                for (index, schema) in items.iter_mut().enumerate() {
                    walk_in_dialect(
                        schema,
                        &child(&child(pointer, key), &index.to_string()),
                        dialect,
                        visitor,
                    );
                }
            }
        }
        for key in SCHEMA_VALUES {
            if draft4 && matches!(*key, "contains" | "if" | "then" | "else" | "propertyNames") {
                continue;
            }
            if legacy
                && matches!(
                    *key,
                    "unevaluatedProperties" | "unevaluatedItems" | "contentSchema"
                )
            {
                continue;
            }
            if *key == "additionalItems" && (!tuple_dialect || !tuple_items) {
                continue;
            }
            if let Some(schema) = object.get_mut(*key) {
                // Legacy dialects already allow boolean additionalProperties.
                // It is a keyword value there, not a standalone boolean schema.
                if *key == "additionalProperties" && schema.is_boolean() {
                    continue;
                }
                if schema.is_object() || schema.is_boolean() {
                    walk_in_dialect(schema, &child(pointer, key), dialect, visitor);
                } else if *key == "items"
                    && tuple_dialect
                    && let Some(items) = schema.as_array_mut()
                {
                    for (index, item) in items.iter_mut().enumerate() {
                        walk_in_dialect(
                            item,
                            &child(&child(pointer, key), &index.to_string()),
                            dialect,
                            visitor,
                        );
                    }
                }
            }
        }
        if tuple_dialect
            && let Some(dependencies) = object
                .get_mut("dependencies")
                .and_then(Value::as_object_mut)
        {
            for (name, schema) in dependencies {
                if schema.is_object() || schema.is_boolean() {
                    walk_in_dialect(
                        schema,
                        &child(&child(pointer, "dependencies"), name),
                        dialect,
                        visitor,
                    );
                }
            }
        }
    }
}

/// A preserved URI is not proof that an unknown vocabulary was converted.
/// Callers may replace the schema explicitly, but cannot acknowledge this
/// blocker away while leaving an unimplemented contract in the output.
pub(crate) fn check_dialects(document: &mut Value, sources: &Sources, trace: &mut Trace) {
    let default = document
        .get("jsonSchemaDialect")
        .and_then(Value::as_str)
        .and_then(octri_parser::SchemaDialect::from_uri)
        .unwrap_or_default();
    let mut check = |schema: &Value, pointer: &str, key: &str| {
        if let Some(uri) = schema.get(key).and_then(Value::as_str)
            && octri_parser::SchemaDialect::from_uri(uri).is_none()
        {
            let at = child(pointer, key);
            trace.record(sources, &sources.root, &at, std::slice::from_ref(&at), "SCHEMA_DIALECT_UNSUPPORTED", Classification::Blocked,
                format!("Schema dialect {uri} needs a matching adapter or an explicitly authored replacement. Preserving its URI does not establish its reference or validation semantics."), Some(&json!(uri)));
        }
    };
    check(document, "", "jsonSchemaDialect");
    crate::api::schemas(document, &mut |schema, pointer| {
        walk_in_dialect(schema, pointer, default, &mut |schema, pointer, _| {
            check(schema, pointer, "$schema")
        });
    });
}

/// Translate Swagger/OAS 3.0 schema semantics into the canonical 3.1 model.
pub(crate) fn to_modern(
    value: &mut Value,
    site: &SchemaSite<'_>,
    swagger: bool,
    trace: &mut Trace,
) {
    walk(value, site.output, &mut |schema, at| {
        modernize_node(schema, at, site, swagger, trace);
    });
}

/// Transform one schema only. Imported resources can cross dialect boundaries;
/// their descendants must be dispatched using each node's physical owner.
fn modernize_node(
    schema: &mut Value,
    at: &str,
    site: &SchemaSite<'_>,
    swagger: bool,
    trace: &mut Trace,
) {
    let sources = site.sources;
    let uri = site.uri;
    let source_at = site.source_at(at);
    let original = schema.clone();
    let Some(map) = schema.as_object_mut() else {
        return;
    };
    if let Some(reference) = map.get("$ref").cloned() {
        if map.len() > 1 {
            trace.record(sources, uri, &source_at, &[at.into()], "IGNORED_REFERENCE_SIBLINGS", Classification::Omitted, "Legacy Reference Object siblings were ignored by the source dialect and are not activated by migration", Some(&original));
            map.clear();
            map.insert("$ref".into(), reference);
        }
        return;
    }
    if map.get("type").and_then(Value::as_str) == Some("file") {
        map.insert("type".into(), json!("string"));
        map.insert("format".into(), json!("binary"));
        trace.record(
            sources,
            uri,
            &source_at,
            &[at.into()],
            "FILE_SCHEMA",
            Classification::Transformed,
            "File payload is represented by an OpenAPI binary string",
            Some(&original),
        );
    }
    if swagger
        && let Some(discriminator) = map
            .get("discriminator")
            .and_then(Value::as_str)
            .map(str::to_owned)
    {
        map.insert(
            "discriminator".into(),
            json!({"propertyName": discriminator}),
        );
    }
    let nullable = map.remove("nullable").or_else(|| {
        if swagger {
            map.remove("x-nullable")
        } else {
            None
        }
    });
    if nullable == Some(Value::Bool(true)) {
        if let Some(kind) = map.get("type").and_then(Value::as_str).map(str::to_owned) {
            map.insert("type".into(), json!([kind, "null"]));
        }
        trace.record(
            sources,
            uri,
            &source_at,
            &[at.into()],
            "NULLABLE_MIGRATION",
            Classification::Transformed,
            "Nullable modifies an explicitly declared type; other constraints retain their meaning",
            Some(&original),
        );
    }
    for (exclusive, bound) in [
        ("exclusiveMinimum", "minimum"),
        ("exclusiveMaximum", "maximum"),
    ] {
        if let Some(Value::Bool(enabled)) = map.get(exclusive).cloned() {
            map.remove(exclusive);
            if enabled && let Some(number) = map.remove(bound) {
                map.insert(exclusive.into(), number);
            }
            trace.record(
                sources,
                uri,
                &child(&source_at, exclusive),
                &[at.into()],
                "EXCLUSIVE_BOUND_MIGRATION",
                Classification::Transformed,
                "Boolean exclusive bound converted to its numeric JSON Schema form",
                Some(&original),
            );
        }
    }
}

/// Examples establish observed types, not required fields, enums or constraints.
pub(crate) fn infer(example: &Value) -> Value {
    match example {
        Value::Null => json!({"type":"null"}),
        Value::Bool(_) => json!({"type":"boolean"}),
        Value::Number(number) => {
            json!({"type":if octri_parser::numbers::is_integer(number) { "integer" } else { "number" }})
        }
        Value::String(_) => json!({"type":"string"}),
        Value::Array(items) => {
            let mut schemas = Vec::new();
            for item in items {
                let schema = infer(item);
                if !schemas.contains(&schema) {
                    schemas.push(schema);
                }
            }
            let items = match schemas.as_slice() {
                [] => json!({}),
                [one] => one.clone(),
                _ => json!({"anyOf":schemas}),
            };
            json!({"type":"array","items":items})
        }
        Value::Object(fields) => {
            json!({"type":"object","properties": fields.iter().map(|(name, value)| (name.clone(), infer(value))).collect::<serde_json::Map<_,_>>()})
        }
    }
}

pub(crate) fn downgrade_schema(
    value: &mut Value,
    site: &SchemaSite<'_>,
    target: &str,
    config: &Config,
    trace: &mut Trace,
    default: octri_parser::SchemaDialect,
    localized_resources: bool,
) {
    let sources = site.sources;
    let uri = site.uri;
    walk_in_dialect(value, site.output, default, &mut |schema, at, dialect| {
        let original = schema.clone();
        if dialect.ignores_reference_siblings()
            && let Some(reference) = schema.get("$ref").cloned()
        {
            if schema.as_object().is_some_and(|map| map.len() > 1) {
                *schema = json!({"$ref":reference});
                trace.record(sources, uri, at, &[at.into()], "IGNORED_REFERENCE_SIBLINGS", Classification::Transformed,
                    "Ignored Draft 7 reference siblings removed without activating their constraints", Some(&original));
            }
            return;
        }
        if schema.is_boolean() {
            if *schema == Value::Bool(true) {
                *schema = json!({});
            } else if target != "2.0" {
                *schema = json!({"not":{}});
            } else {
                // Swagger 2.0 supports allOf. Disjoint primitive types form an
                // unsatisfiable intersection without unsupported not/anyOf.
                *schema = json!({"allOf":[{"type":"string"},{"type":"number"}]});
            }
            trace.record(sources,uri,at,&[at.into()],"BOOLEAN_SCHEMA_DOWNGRADE",Classification::Transformed,"Boolean schema represented by equivalent constraints in the selected legacy dialect",Some(&original));
            return;
        }
        let Some(map) = schema.as_object_mut() else {
            return;
        };
        if localized_resources {
            let identifier = if dialect == octri_parser::SchemaDialect::Draft4 {
                "id"
            } else {
                "$id"
            };
            let mut removed = serde_json::Map::new();
            for key in ["$schema", identifier, "$anchor"] {
                if key == "$anchor" && dialect.ignores_reference_siblings() {
                    continue;
                }
                if let Some(value) = map.remove(key) {
                    removed.insert(key.into(), value);
                }
            }
            if !removed.is_empty() {
                trace.record(sources,uri,at,&[at.into()],"SCHEMA_RESOURCE_LOCALIZED",Classification::Transformed,
                    "Static schema resource identity replaced by artifact-local references before legacy emission; original identifiers and dialect remain in source evidence",Some(&Value::Object(removed)));
            }
        }
        if map.len() > 1
            && let Some(reference) = map.remove("$ref")
        {
            let conjunction = map.entry("allOf").or_insert_with(|| json!([]));
            if let Some(items) = conjunction.as_array_mut() {
                items.push(json!({"$ref":reference}));
            }
            trace.record(
                sources,
                uri,
                at,
                &[at.into()],
                "REFERENCE_CONJUNCTION",
                Classification::Transformed,
                "Modern schema reference siblings preserved through a legacy allOf conjunction",
                Some(&original),
            );
        }
        if let Some(types) = map.get("type").and_then(Value::as_array).cloned() {
            let non_null: Vec<_> = types
                .iter()
                .filter(|kind| kind.as_str() != Some("null"))
                .cloned()
                .collect();
            if types.iter().any(|kind| kind.as_str() == Some("null")) && non_null.len() == 1 {
                map.insert("type".into(), non_null[0].clone());
                if target == "2.0" {
                    unsupported(
                        &SchemaSite::at(sources, uri, &child(at, "type")),
                        target,
                        config,
                        trace,
                        &original,
                        "Swagger 2.0 does not define nullable types; x-nullable is only a vendor convention",
                    );
                    map.insert("x-nullable".into(), Value::Bool(true));
                } else {
                    map.insert("nullable".into(), Value::Bool(true));
                }
                trace.record(
                    sources,
                    uri,
                    at,
                    &[at.into()],
                    "NULLABLE_DOWNGRADE",
                    Classification::Transformed,
                    "Nullable union represented in the selected legacy dialect",
                    Some(&original),
                );
            } else if target != "2.0" {
                map.remove("type");
                let alternatives = types
                    .iter()
                    .map(|kind| {
                        if kind == "null" {
                            json!({"type":"string","nullable":true,"enum":[null]})
                        } else {
                            json!({"type":kind})
                        }
                    })
                    .collect::<Vec<_>>();
                let conjunction = map.entry("allOf").or_insert_with(|| json!([]));
                if let Some(items) = conjunction.as_array_mut() {
                    items.push(json!({"anyOf":alternatives}));
                }
                trace.record(
                    sources,
                    uri,
                    at,
                    &[at.into()],
                    "TYPE_UNION_DOWNGRADE",
                    Classification::Transformed,
                    "Type union preserved as legacy anyOf without changing sibling constraints",
                    Some(&original),
                );
            } else {
                unsupported(
                    &SchemaSite::at(sources, uri, &child(at, "type")),
                    target,
                    config,
                    trace,
                    &original,
                    "This union type cannot be represented by the target's type keyword",
                );
            }
        } else if map.get("type").and_then(Value::as_str) == Some("null") {
            if target != "2.0" {
                map.insert("type".into(), json!("string"));
                map.insert("nullable".into(), json!(true));
                let constraint = json!({"type":"string","nullable":true,"enum":[null]});
                let conjunction = map.entry("allOf").or_insert_with(|| json!([]));
                if let Some(items) = conjunction.as_array_mut() {
                    items.push(constraint);
                }
            } else {
                unsupported(
                    &SchemaSite::at(sources, uri, &child(at, "type")),
                    target,
                    config,
                    trace,
                    &original,
                    "A null-only type cannot be represented exactly by this target",
                );
            }
        }
        for (exclusive, bound) in [
            ("exclusiveMinimum", "minimum"),
            ("exclusiveMaximum", "maximum"),
        ] {
            if let Some(number) = map
                .get(exclusive)
                .filter(|number| number.is_number())
                .cloned()
            {
                let stronger = match (
                    number.as_number(),
                    map.get(bound).and_then(Value::as_number),
                ) {
                    (Some(exclusive_value), Some(inclusive_value)) => {
                        let order = crate::numbers::compare(exclusive_value, inclusive_value);
                        if bound == "minimum" {
                            !order.is_lt()
                        } else {
                            !order.is_gt()
                        }
                    }
                    _ => true,
                };
                if stronger {
                    map.insert(bound.into(), number);
                    map.insert(exclusive.into(), Value::Bool(true));
                } else {
                    map.remove(exclusive);
                }
                trace.record(
                    sources,
                    uri,
                    &child(at, exclusive),
                    &[at.into()],
                    "EXCLUSIVE_BOUND_DOWNGRADE",
                    Classification::Transformed,
                    "Effective inclusive/exclusive bound selected using exact decimal comparison",
                    Some(&original),
                );
            }
        }
        if let Some(constant) = map.remove("const") {
            if map.get("enum").is_some_and(|values| {
                !values.as_array().is_some_and(|values| {
                    values
                        .iter()
                        .any(|value| octri_parser::numbers::equivalent(value, &constant))
                })
            }) {
                // Empty intersection is false, expressible in both legacy
                // dialects. An empty enum itself is invalid in OpenAPI 3.0.
                map.clear();
                if target == "2.0" {
                    map.insert("allOf".into(), json!([{"type":"string"},{"type":"number"}]));
                } else {
                    map.insert("not".into(), json!({}));
                }
            } else {
                map.insert("enum".into(), json!([constant]));
            }
            trace.record(sources, uri, &child(at, "const"), &[at.into()], "CONST_DOWNGRADE", Classification::Transformed, "Const and enum intersection preserved as a singleton enum or an unsatisfiable schema", Some(&original));
        }
        let modern = [
            "$schema",
            "$id",
            "id",
            "$anchor",
            "$dynamicRef",
            "$dynamicAnchor",
            "$recursiveRef",
            "$recursiveAnchor",
            "$defs",
            "if",
            "then",
            "else",
            "unevaluatedProperties",
            "unevaluatedItems",
            "dependentSchemas",
            "dependentRequired",
            "prefixItems",
            "dependencies",
            "definitions",
            "additionalItems",
            "contains",
            "minContains",
            "maxContains",
            "propertyNames",
            "contentEncoding",
            "contentMediaType",
            "contentSchema",
            "patternProperties",
        ];
        if map.get("items").is_some_and(Value::is_array) {
            let location = child(at, "items");
            let value = map["items"].clone();
            unsupported(
                &SchemaSite::at(sources, uri, &location),
                target,
                config,
                trace,
                &value,
                "Tuple validation has no equivalent in this target. Supply an authored replacement or explicitly remove tuple constraints.",
            );
            if config.drop_unsupported.contains(&location) {
                map.insert("items".into(), json!({}));
            }
        }
        for key in modern {
            if let Some(value) = map.get(key).cloned() {
                let location = child(at, key);
                unsupported(
                    &SchemaSite::at(sources, uri, &location),
                    target,
                    config,
                    trace,
                    &value,
                    "JSON Schema keyword needs a reviewed replacement or explicit lossy removal for this dialect",
                );
                if config.drop_unsupported.contains(&location) {
                    map.remove(key);
                }
            }
        }
        if target == "2.0" {
            for key in ["anyOf", "oneOf", "not", "writeOnly", "deprecated"] {
                if let Some(value) = map.get(key).cloned() {
                    let location = child(at, key);
                    unsupported(
                        &SchemaSite::at(sources, uri, &location),
                        target,
                        config,
                        trace,
                        &value,
                        "Schema feature is outside Swagger 2.0",
                    );
                    if config.drop_unsupported.contains(&location) {
                        map.remove(key);
                    }
                }
            }
            if let Some(discriminator) = map.get("discriminator").cloned() {
                if discriminator.get("mapping").is_some() {
                    unsupported(
                        &SchemaSite::at(sources, uri, &child(at, "discriminator")),
                        target,
                        config,
                        trace,
                        &discriminator,
                        "Swagger 2.0 cannot preserve an explicit discriminator mapping",
                    );
                }
                if let Some(name) = discriminator.get("propertyName") {
                    map.insert("discriminator".into(), name.clone());
                }
            }
        }
    });
}

fn unsupported(
    site: &SchemaSite<'_>,
    target: &str,
    config: &Config,
    trace: &mut Trace,
    evidence: &Value,
    message: &str,
) {
    let sources = site.sources;
    let uri = site.uri;
    let at = site.source;
    let dropped = config.drop_unsupported.contains(at);
    trace.record(
        sources,
        uri,
        at,
        &[at.into()],
        &format!("TARGET_{target}_UNREPRESENTABLE"),
        if dropped {
            Classification::Omitted
        } else {
            Classification::Blocked
        },
        message,
        Some(evidence),
    );
}
