//! Postman 3 uses directly typed items rather than the v2 request wrapper.
//! Keep physical pointers in that original tree; never rewrite exports to v2.

use super::{Context, description, variables_from};
use crate::adapters::{
    common::{object, unknown_fields},
    http::{self, Auth, Observation, Payload, Response},
    references::{Schemas, parse_schema_text},
};
use crate::{
    Classification, Config, ConversionError, Result,
    source::{Sources, child},
    trace::Trace,
};
use octri_parser::{Address, SchemaDialect};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

mod auth;
use auth::inherited_auth;

#[derive(Clone)]
struct InheritedAuth {
    value: Arc<Value>,
    pointer: String,
}

pub(super) fn convert(
    document: &Value,
    sources: &mut Sources,
    config: &Config,
    trace: &mut Trace,
) -> Result<Value> {
    let mut variables = BTreeMap::new();
    let mut active_names = std::collections::BTreeSet::new();
    if let Some(entries) = document.get("variables") {
        let entries = entries.as_array().ok_or_else(|| {
            ConversionError::new("POSTMAN_VARIABLE", "Collection variables must be an array")
        })?;
        for variable in entries {
            if variable["disabled"] == true {
                continue;
            }
            let name = variable["key"].as_str().ok_or_else(|| {
                ConversionError::new("POSTMAN_VARIABLE", "Variable needs a string key")
            })?;
            // Secret references are capabilities, not inline values. Only the
            // explicitly supplied environment/configuration can resolve them.
            if !active_names.insert(name) {
                return Err(ConversionError::new(
                    "POSTMAN_VARIABLE",
                    format!("Duplicate active variable {name}"),
                ));
            }
            if variable["secret"] != true {
                let value = match variable.get("value") {
                    None => "",
                    Some(Value::String(value)) => value,
                    _ => {
                        return Err(ConversionError::new(
                            "POSTMAN_VARIABLE",
                            format!("Variable {name} needs a string value"),
                        ));
                    }
                };
                variables.insert(name.into(), value.into());
            }
        }
    }
    let mut priority = BTreeMap::new();
    if let Some(name) = &config.environment_uri {
        let uri = sources.relative(&sources.root, name)?;
        // The shared entry point loads the environment before this immutable
        // adapter runs, so supplied files never gain an I/O capability.
        let environment = sources.documents.get(&uri).ok_or_else(|| {
            ConversionError::new("POSTMAN_VARIABLE", "Environment was not parsed")
        })?;
        priority.extend(variables_from(environment.document.get("values"), "key")?);
    }
    priority.extend(config.variables.clone());
    variables.extend(priority.clone());
    let mut context = Context {
        sources,
        config,
        trace,
        variables,
        priority_variables: priority,
        requests: Vec::new(),
        schemas: Schemas::default(),
    };
    metadata(&mut context, document, "");
    let inherited = InheritedAuth {
        value: Arc::new(document.get("auth").cloned().unwrap_or(Value::Null)),
        pointer: "/auth".into(),
    };
    items(
        &mut context,
        document.get("items"),
        "/items",
        &[],
        &inherited,
        0,
    )?;
    unknown_fields(
        document,
        &[
            "id",
            "name",
            "description",
            "$kind",
            "variables",
            "scripts",
            "auth",
            "uses",
            "items",
        ],
        context.sources,
        &context.sources.root,
        "",
        context.trace,
    );
    let title = document["name"].as_str().ok_or_else(|| {
        ConversionError::new("POSTMAN_COLLECTION", "Collection needs a string name")
    })?;
    let mut builder = http::Builder::new(title, context.sources, config, context.trace);
    for request in context.requests {
        builder.add(request)?;
    }
    let mut output = builder.finish();
    let schemas = context
        .schemas
        .finish(context.sources, Default::default())?;
    if !schemas.is_empty() {
        output["components"]["schemas"] = Value::Object(schemas);
    }
    if let Some(description) = document["description"].as_str() {
        output["info"]["description"] = json!(description);
    }
    Ok(output)
}

fn items(
    context: &mut Context<'_>,
    value: Option<&Value>,
    pointer: &str,
    tags: &[String],
    inherited: &InheritedAuth,
    depth: usize,
) -> Result<()> {
    if depth > context.config.limits.max_depth {
        return Err(ConversionError::new(
            "POSTMAN_DEPTH",
            "Collection nesting exceeds configured limit",
        ));
    }
    let Some(value) = value else {
        return Ok(());
    };
    let entries = value.as_array().ok_or_else(|| {
        ConversionError::new(
            "POSTMAN_ITEMS",
            format!("Expected items array at {pointer}"),
        )
    })?;
    for (index, item) in entries.iter().enumerate() {
        object(item, "Postman item")?;
        let at = child(pointer, &index.to_string());
        let auth = item
            .get("auth")
            .filter(|auth| !auth.is_null() && auth["type"] != "inherit")
            .map_or_else(
                || inherited.clone(),
                |auth| InheritedAuth {
                    value: Arc::new(auth.clone()),
                    pointer: child(&at, "auth"),
                },
            );
        metadata(context, item, &at);
        match item["$kind"].as_str() {
            Some("collection") => {
                let mut tags = tags.to_vec();
                tags.push(required_text(item, "name", &at)?.into());
                items(
                    context,
                    item.get("items"),
                    &child(&at, "items"),
                    &tags,
                    &auth,
                    depth + 1,
                )?;
                unknown_fields(
                    item,
                    &[
                        "id",
                        "name",
                        "description",
                        "$kind",
                        "scripts",
                        "auth",
                        "items",
                    ],
                    context.sources,
                    &context.sources.root,
                    &at,
                    context.trace,
                );
            }
            Some("http-request") => {
                let request = request(context, item, &at, tags, &auth)?;
                context.requests.push(request);
                if context.requests.len() > context.config.limits.max_nodes {
                    return Err(ConversionError::new(
                        "LIMIT_OPERATIONS",
                        "Too many requests",
                    ));
                }
            }
            Some(kind) => {
                context.record(&at,"POSTMAN_PROTOCOL_MAPPING_REQUIRED",Classification::Blocked,
                    format!("Postman {kind} item requires its protocol specification and an explicit HTTP mapping. Supply that specification to the corresponding converter adapter; no endpoint is inferred from a client request."),Some(item));
            }
            None => {
                return Err(ConversionError::new(
                    "POSTMAN_ITEM",
                    format!("Item needs $kind at {at}"),
                ));
            }
        }
    }
    Ok(())
}

fn request(
    context: &mut Context<'_>,
    item: &Value,
    at: &str,
    tags: &[String],
    inherited: &InheritedAuth,
) -> Result<Observation> {
    let name: String = required_text(item, "name", at)?.into();
    let method = required_text(item, "method", at)?.into();
    let url_at = child(at, "url");
    let mut url = context.expand(required_text(item, "url", at)?, &url_at);
    if let Some(entries) = item.get("queryParams") {
        let entries = entries
            .as_array()
            .ok_or_else(|| ConversionError::new("POSTMAN_URL", "queryParams must be an array"))?;
        let fragment = url.split_once('#').map(|(_, fragment)| fragment.to_owned());
        url = url.split(['?', '#']).next().unwrap_or("").into();
        let mut pairs = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            if entry["disabled"] == true {
                continue;
            }
            let location = child(&child(at, "queryParams"), &index.to_string());
            let key = entry.get("key").and_then(Value::as_str).ok_or_else(|| {
                ConversionError::new(
                    "POSTMAN_URL",
                    format!(
                        "Unnamed query parameter requires an authored HTTP contract at {location}"
                    ),
                )
            })?;
            pairs.push((
                context.expand(key, &child(&location, "key")),
                context.expand(
                    entry["value"].as_str().unwrap_or(""),
                    &child(&location, "value"),
                ),
            ));
            type_annotation(context, entry, &location);
        }
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish();
        if !query.is_empty() {
            url.push('?');
            url.push_str(&query);
        }
        if let Some(fragment) = fragment {
            url.push('#');
            url.push_str(&fragment);
        }
    }
    let headers = context.headers(item.get("headers"), &child(at, "headers"))?;
    annotations(context, item.get("headers"), &child(at, "headers"));
    let mut path_variables = BTreeMap::new();
    if let Some(entries) = item.get("pathVariables") {
        for (index, entry) in entries
            .as_array()
            .ok_or_else(|| ConversionError::new("POSTMAN_URL", "pathVariables must be an array"))?
            .iter()
            .enumerate()
        {
            if entry["disabled"] == true {
                continue;
            }
            let location = child(&child(at, "pathVariables"), &index.to_string());
            let key = required_text(entry, "key", &location)?;
            let value = context.expand(
                required_text(entry, "value", &location)?,
                &child(&location, "value"),
            );
            if path_variables.insert(key.into(), value).is_some() {
                return Err(ConversionError::new(
                    "POSTMAN_URL",
                    format!("Duplicate path variable {key}"),
                ));
            }
            type_annotation(context, entry, &location);
        }
    }
    let auth = inherited_auth(context, inherited, at, &name)?;
    let body = context.body(item.get("body"), &headers, &child(at, "body"))?;
    let mut responses = Vec::new();
    if let Some(examples) = item.get("examples") {
        for (index, example) in examples
            .as_array()
            .ok_or_else(|| ConversionError::new("POSTMAN_RESPONSE", "examples must be an array"))?
            .iter()
            .enumerate()
        {
            let example_at = child(&child(at, "examples"), &index.to_string());
            if example["$kind"] != "http-example" {
                return Err(ConversionError::new(
                    "POSTMAN_RESPONSE",
                    format!("Expected http-example at {example_at}"),
                ));
            }
            let location = child(&example_at, "response");
            let response = &example["response"];
            let code = response["statusCode"]
                .as_u64()
                .filter(|code| (100..=599).contains(code))
                .ok_or_else(|| {
                    ConversionError::new(
                        "POSTMAN_RESPONSE",
                        format!("Response needs an HTTP status code at {location}"),
                    )
                })?;
            let headers = context.headers(response.get("headers"), &child(&location, "headers"))?;
            annotations(
                context,
                response.get("headers"),
                &child(&location, "headers"),
            );
            let payload =
                context.body(response.get("body"), &headers, &child(&location, "body"))?;
            responses.push(Response {
                status: code.to_string(),
                description: required_text(example, "name", &example_at)?.into(),
                headers,
                payload,
                pointer: location,
            });
            for key in ["request", "auth", "description"] {
                if let Some(value) = example.get(key) {
                    context.record(&child(&example_at,key),"POSTMAN_EXAMPLE_CONTEXT",Classification::Omitted,"Example-specific request context remains in source; it does not override the parent operation contract",Some(value));
                }
            }
            unknown_fields(
                example,
                &[
                    "id",
                    "name",
                    "description",
                    "auth",
                    "$kind",
                    "request",
                    "response",
                ],
                context.sources,
                &context.sources.root,
                &example_at,
                context.trace,
            );
            unknown_fields(
                response,
                &["statusCode", "statusText", "headers", "body"],
                context.sources,
                &context.sources.root,
                &child(&example_at, "response"),
                context.trace,
            );
        }
    }
    unknown_fields(
        item,
        &[
            "id",
            "name",
            "description",
            "url",
            "method",
            "headers",
            "queryParams",
            "pathVariables",
            "body",
            "auth",
            "$kind",
            "examples",
            "settings",
            "scripts",
            "uses",
            "usedBy",
        ],
        context.sources,
        &context.sources.root,
        at,
        context.trace,
    );
    Ok(Observation {
        pointer: at.into(),
        name,
        description: description(item.get("description")),
        method,
        url,
        headers,
        path_variables,
        body,
        responses,
        tags: tags.to_vec(),
        auth,
    })
}

fn metadata(context: &mut Context<'_>, value: &Value, at: &str) {
    if let Some(scripts) = value.get("scripts") {
        if let Some(scripts) = scripts.as_array() {
            for (index, script) in scripts.iter().enumerate() {
                context.record(&child(&child(at,"scripts"),&index.to_string()),"SCRIPT_NOT_EXECUTED",Classification::Omitted,"Postman script retained verbatim. Dynamic request mutations require authored values; scripts and package imports never execute",Some(script));
            }
        } else {
            context.record(
                &child(at, "scripts"),
                "POSTMAN_SCRIPTS",
                Classification::Blocked,
                "Scripts must be an array",
                Some(scripts),
            );
        }
    }
    for key in ["settings", "uses", "usedBy"] {
        if let Some(value) = value.get(key) {
            context.record(&child(at,key),"POSTMAN_RUNTIME_SETTINGS",Classification::Omitted,"Client runtime settings and linked resources remain in source; no external resource is retrieved",Some(value));
        }
    }
}

fn annotations(context: &mut Context<'_>, value: Option<&Value>, pointer: &str) {
    for (index, entry) in value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if entry["disabled"] != true {
            type_annotation(context, entry, &child(pointer, &index.to_string()));
        }
    }
}

fn type_annotation(context: &mut Context<'_>, value: &Value, pointer: &str) {
    if let Some(description) = value.get("description") {
        context.record(&child(pointer,"description"),"POSTMAN_PARAMETER_DESCRIPTION",Classification::Omitted,
            "Parameter documentation remains in source; review or author the corresponding OpenAPI parameter description",Some(description));
    }
    if let Some(annotation) = value.get("$type") {
        context.record(&child(pointer,"$type"),"POSTMAN_PARAMETER_TYPE",Classification::Omitted,"Postman parameter type annotation retained in source. Observed wire strings do not establish serialization or coercion; author a parameter schema before handoff",Some(annotation));
    }
}

fn required_text<'a>(value: &'a Value, key: &str, pointer: &str) -> Result<&'a str> {
    value.get(key).and_then(Value::as_str).ok_or_else(|| {
        ConversionError::new(
            "POSTMAN_ITEM",
            format!("Expected string {key} at {pointer}"),
        )
    })
}

pub(super) fn authored_body_type(
    context: &mut Context<'_>,
    value: &Value,
    pointer: &str,
    payload: Option<Payload>,
) -> Result<Option<Payload>> {
    let Some(annotation) = value.get("$type") else {
        return Ok(payload);
    };
    let at = child(pointer, "$type");
    let text = annotation
        .as_str()
        .ok_or_else(|| ConversionError::new("POSTMAN_BODY_TYPE", "$type must be a string"))?;
    let mut payload = payload.ok_or_else(|| {
        ConversionError::new(
            "POSTMAN_BODY_TYPE",
            "Body type annotation has no body to describe",
        )
    })?;
    let schema = if matches!(
        text,
        "object" | "array" | "string" | "number" | "integer" | "boolean" | "null"
    ) {
        json!({"type":text})
    } else {
        let address = Address {
            uri: context.sources.root.clone(),
            pointer: at.clone(),
        };
        let parsed = parse_schema_text(text, &address, context.sources)?;
        if !parsed.is_object() && !parsed.is_boolean() {
            return Err(ConversionError::new(
                "POSTMAN_BODY_TYPE",
                "$type must name a JSON type or encode a schema object",
            ));
        }
        if parsed
            .get("$schema")
            .and_then(Value::as_str)
            .is_some_and(|uri| SchemaDialect::from_uri(uri).is_none())
        {
            context.record(&at,"POSTMAN_BODY_TYPE_DIALECT",Classification::Blocked,
                "Declared body schema dialect requires a matching parser implementation or authored replacement",Some(annotation));
            return Ok(Some(payload));
        }
        let mut schema = parsed;
        if crate::schema::has_resource_scope(&mut schema) {
            // Collection pointers and decoded-string offsets cannot become
            // schema reference targets. Scoped annotations need a resource.
            context.schemas.import_serialized(
                schema,
                address,
                context.sources,
                SchemaDialect::Draft202012,
            )?
        } else {
            // Avoid introducing resource keywords into portable inline schemas.
            schema
        }
    };
    payload.schema = schema;
    payload.inferred = false;
    context.record(&at,"POSTMAN_AUTHORED_TYPE",Classification::Transformed,
        "Authored body type applied through the shared parser; output receives strict schema and example validation",Some(annotation));
    Ok(Some(payload))
}
