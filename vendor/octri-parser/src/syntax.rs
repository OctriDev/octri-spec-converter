//! Context-aware traversal. Literal examples and extension payloads are never executable references.

use crate::SchemaDialect;
use crate::pointer::child;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Context {
    Api,
    ApiMap,
    Schema,
    SchemaMap,
    SchemaArray,
    Dependencies,
    Literal,
}

pub(crate) fn root_context(value: &Value) -> Context {
    if value.get("openapi").is_some() || value.get("swagger").is_some() || value.get("in").is_some()
    {
        return Context::Api;
    }
    let schema_type = value.get("type").is_some_and(|kind| {
        kind.is_array()
            || kind.as_str().is_some_and(|kind| {
                matches!(
                    kind,
                    "object" | "string" | "number" | "integer" | "boolean" | "null" | "array"
                )
            })
    });
    if value.is_boolean()
        || schema_type
        || [
            "$schema",
            "$id",
            "$defs",
            "properties",
            "allOf",
            "anyOf",
            "oneOf",
            "$anchor",
            "$dynamicAnchor",
            "$recursiveAnchor",
        ]
        .iter()
        .any(|key| value.get(key).is_some())
    {
        Context::Schema
    } else {
        Context::Api
    }
}

/// Foreign JSON files may be schema documents or containers of schema roots.
/// Do not index an entire API/protocol container as a schema merely because a
/// schema reference selected one of its JSON Pointers.
pub(crate) fn schema_document(value: &Value, dialect: SchemaDialect) -> bool {
    value.is_boolean()
        || value.as_object().is_some_and(|map| {
            map.is_empty()
                || map.contains_key("$ref")
                || map.iter().any(|(key, child)| {
                    next_schema(
                        Context::Schema,
                        key,
                        child,
                        value.get("items").is_some_and(Value::is_array),
                        crate::Dialect::OpenApi31,
                        dialect,
                    ) != Context::Literal
                })
        })
        || root_context(value) == Context::Schema
}

pub(crate) fn next(context: Context, key: &str, value: &Value) -> Context {
    match context {
        Context::Literal => Context::Literal,
        Context::SchemaMap | Context::SchemaArray => Context::Schema,
        Context::Dependencies => {
            if value.is_object() || value.is_boolean() {
                Context::Schema
            } else {
                Context::Literal
            }
        }
        Context::ApiMap => Context::Api,
        Context::Schema => match key {
            "properties" | "patternProperties" | "$defs" | "definitions" | "dependentSchemas" => {
                Context::SchemaMap
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => Context::SchemaArray,
            "items"
            | "additionalProperties"
            | "additionalItems"
            | "contains"
            | "not"
            | "if"
            | "then"
            | "else"
            | "propertyNames"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            | "contentSchema" => Context::Schema,
            _ => Context::Literal,
        },
        Context::Api => match key {
            "schemas" | "definitions" => Context::SchemaMap,
            "schema" | "itemSchema" => Context::Schema,
            "example" | "value" | "dataValue" | "serializedValue" | "default" | "enum"
            | "const" => Context::Literal,
            key if key.starts_with("x-") => Context::Literal,
            "paths"
            | "webhooks"
            | "callbacks"
            | "responses"
            | "content"
            | "examples"
            | "parameters"
            | "headers"
            | "requestBodies"
            | "links"
            | "securitySchemes"
            | "pathItems"
            | "mediaTypes"
            | "additionalOperations"
                if value.is_object() =>
            {
                Context::ApiMap
            }
            _ => Context::Api,
        },
    }
}

pub(crate) fn visit_schema(
    value: &Value,
    pointer: &str,
    context: Context,
    dialect: crate::Dialect,
    inherited: SchemaDialect,
    visitor: &mut impl FnMut(&Value, &str, Context, SchemaDialect),
) {
    if context == Context::Literal {
        return;
    }
    let context = if context == Context::Api && root_context(value) == Context::Schema {
        Context::Schema
    } else {
        context
    };
    let schema_dialect = if context == Context::Schema && !dialect.modern_schema() {
        SchemaDialect::Draft4
    } else if context == Context::Schema {
        SchemaDialect::declared(value).unwrap_or(inherited)
    } else {
        inherited
    };
    visitor(value, pointer, context, schema_dialect);
    // Draft 7 Reference Objects ignore every sibling, including schema-shaped
    // objects and identifiers. A later explicit reference may select a sibling
    // as its own root; merely appearing here must not activate it.
    if context == Context::Schema
        && schema_dialect.ignores_reference_siblings()
        && value.get("$ref").is_some()
    {
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, child_value) in map {
                visit_schema(
                    child_value,
                    &child(pointer, key),
                    next_schema(
                        context,
                        key,
                        child_value,
                        value.get("items").is_some_and(Value::is_array),
                        dialect,
                        schema_dialect,
                    ),
                    dialect,
                    schema_dialect,
                    visitor,
                );
            }
        }
        Value::Array(values) => {
            for (i, value) in values.iter().enumerate() {
                visit_schema(
                    value,
                    &child(pointer, &i.to_string()),
                    if context == Context::SchemaArray {
                        Context::Schema
                    } else {
                        context
                    },
                    dialect,
                    schema_dialect,
                    visitor,
                );
            }
        }
        _ => {}
    }
}

/// Schema positions differ between drafts. Values of unknown keywords remain
/// literal data even when they contain `$ref` or `$id` keys.
pub(crate) fn next_schema(
    context: Context,
    key: &str,
    value: &Value,
    tuple_items: bool,
    dialect: crate::Dialect,
    schema_dialect: SchemaDialect,
) -> Context {
    if context == Context::Schema
        && schema_dialect == SchemaDialect::Draft202012
        && key == "additionalItems"
    {
        return Context::Literal;
    }
    if context == Context::Schema && schema_dialect == SchemaDialect::Draft201909 {
        return match key {
            "prefixItems" => Context::Literal,
            "items" if value.is_array() => Context::SchemaArray,
            "additionalItems" if !tuple_items => Context::Literal,
            // The standard meta-schema reserves these legacy locations for
            // compatibility. Index schemas there without evaluating assertions.
            "dependencies" => Context::Dependencies,
            _ => next_in_dialect(context, key, value, dialect),
        };
    }
    if context != Context::Schema || !schema_dialect.ignores_reference_siblings() {
        return next_in_dialect(context, key, value, dialect);
    }
    if schema_dialect == SchemaDialect::Draft4
        && matches!(key, "contains" | "if" | "then" | "else" | "propertyNames")
    {
        return Context::Literal;
    }
    match key {
        "properties" | "patternProperties" | "definitions" => Context::SchemaMap,
        "dependencies" => Context::Dependencies,
        "allOf" | "anyOf" | "oneOf" => Context::SchemaArray,
        "items" if value.is_array() => Context::SchemaArray,
        "additionalItems" if !tuple_items => Context::Literal,
        "items"
        | "additionalItems"
        | "additionalProperties"
        | "contains"
        | "not"
        | "if"
        | "then"
        | "else"
        | "propertyNames" => Context::Schema,
        _ => Context::Literal,
    }
}

/// Only the effective schema dialect can activate a reference keyword. API
/// Reference Objects always use `$ref`, independently of their schema default.
pub(crate) fn reference_keywords(
    context: Context,
    dialect: SchemaDialect,
) -> &'static [&'static str] {
    match (context, dialect) {
        (Context::Schema, SchemaDialect::Draft201909) => &["$ref", "$recursiveRef"],
        (Context::Schema, SchemaDialect::Draft202012) => &["$ref", "$dynamicRef"],
        (Context::Api | Context::Schema, _) => &["$ref"],
        _ => &[],
    }
}

pub(crate) fn reference_slot(kind: &str) -> Option<usize> {
    match kind {
        "$ref" => Some(0),
        "$dynamicRef" => Some(1),
        "$recursiveRef" => Some(2),
        _ => None,
    }
}

pub(crate) fn anchor_keywords(dialect: SchemaDialect) -> &'static [&'static str] {
    match dialect {
        SchemaDialect::Draft4 | SchemaDialect::Draft7 => &[],
        SchemaDialect::Draft201909 => &["$anchor"],
        SchemaDialect::Draft202012 => &["$anchor", "$dynamicAnchor"],
    }
}

/// Swagger Response.examples maps MIME types directly to arbitrary instances.
/// OpenAPI 3 examples maps names to Example/Reference Objects instead.
pub(crate) fn next_in_dialect(
    context: Context,
    key: &str,
    value: &Value,
    dialect: crate::Dialect,
) -> Context {
    if context == Context::Api && key == "examples" && dialect == crate::Dialect::Swagger2 {
        Context::Literal
    } else {
        next(context, key, value)
    }
}

pub(crate) fn is_path_item(pointer: &str, value: &Value) -> bool {
    let parts: Vec<_> = pointer.split('/').collect();
    matches!(
        parts.as_slice(),
        ["", "paths" | "webhooks", _] | ["", "components", "pathItems", _]
    ) || pointer
        .rsplit_once("/callbacks/")
        .is_some_and(|(_, tail)| tail.split('/').count() == 2)
        || pointer.is_empty()
            && value.as_object().is_some_and(|map| {
                [
                    "get", "put", "post", "delete", "patch", "options", "head", "trace", "query",
                ]
                .iter()
                .any(|key| map.contains_key(*key))
            })
}

pub(crate) fn schema_reference_sibling_allowed(
    context: Context,
    dialect: crate::Dialect,
    schema_dialect: SchemaDialect,
    pointer: &str,
    value: &Value,
    key: &str,
) -> bool {
    key == "$ref"
        // Retain the dialect selector when serializing a Draft 7 reference at
        // an OpenAPI schema root. It never activates validation siblings.
        || context == Context::Schema && dialect.modern_schema() && key == "$schema"
        || context == Context::Schema && dialect.modern_schema() && !schema_dialect.ignores_reference_siblings()
        || is_path_item(pointer, value)
        || context == Context::Api
            && dialect.modern_schema()
            && matches!(key, "summary" | "description")
}
