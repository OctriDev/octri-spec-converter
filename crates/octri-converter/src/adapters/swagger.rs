//! Swagger 2.0 -> canonical OpenAPI 3.1. Transport and schema changes are separate.

use super::common::{extensions, method, object, strings, unknown_fields};
use crate::schema::{self, SchemaSite};
use crate::source::{Sources, child};
use crate::trace::Trace;
use crate::{Classification, Config, ConversionError, Result};
use serde_json::{Map, Value, json};

const PARAMETER_FIELDS: &[&str] = &["name", "in", "description", "required", "allowEmptyValue"];
const SCHEMA_FIELDS: &[&str] = &[
    "type",
    "format",
    "items",
    "default",
    "maximum",
    "exclusiveMaximum",
    "minimum",
    "exclusiveMinimum",
    "maxLength",
    "minLength",
    "pattern",
    "maxItems",
    "minItems",
    "uniqueItems",
    "enum",
    "multipleOf",
    "x-nullable",
];

pub(crate) fn convert(
    source: &Value,
    sources: &Sources,
    config: &Config,
    trace: &mut Trace,
) -> Result<Value> {
    let root = object(source, "Swagger document")?;
    if source["swagger"] != "2.0" {
        return Err(ConversionError::new(
            "UNSUPPORTED_SWAGGER",
            "This adapter accepts Swagger 2.0 declarations",
        ));
    }
    let uri = &sources.root;
    let mut output = json!({"openapi":"3.1.2","info": source.get("info").cloned().unwrap_or_else(||json!({})),"paths":{},"components":{}});
    for key in ["tags", "externalDocs", "security"] {
        if let Some(value) = root.get(key) {
            output[key] = value.clone();
        }
    }
    if let Some(map) = output.as_object_mut() {
        extensions(root, map);
    }
    output["servers"] = servers(source, sources, config, trace);
    if let Some(definitions) = root.get("definitions").and_then(Value::as_object) {
        let mut schemas = Map::new();
        for (name, schema) in definitions {
            let from = child("/definitions", name);
            let to = child("/components/schemas", name);
            let mut schema = schema.clone();
            schema::to_modern(
                &mut schema,
                &SchemaSite {
                    sources,
                    uri,
                    source: &from,
                    output: &to,
                },
                true,
                trace,
            );
            schemas.insert(name.clone(), schema);
            trace.record(
                sources,
                uri,
                &from,
                &[to],
                "REUSABLE_SCHEMA",
                Classification::Transformed,
                "Reusable schema moved into components",
                Some(&definitions[name]),
            );
        }
        output["components"]["schemas"] = Value::Object(schemas);
    }
    if let Some(security) = root.get("securityDefinitions").and_then(Value::as_object) {
        let mut schemes = Map::new();
        for (name, definition) in security {
            schemes.insert(
                name.clone(),
                security_scheme(
                    definition,
                    sources,
                    &child("/securityDefinitions", name),
                    trace,
                ),
            );
        }
        output["components"]["securitySchemes"] = Value::Object(schemes);
    }
    if let Some(parameters) = root.get("parameters").and_then(Value::as_object) {
        for (name, value) in parameters {
            let pointer = child("/parameters", name);
            let (value, original) = resolve_parameter(value, &pointer, source)?;
            if matches!(value["in"].as_str(), Some("body" | "formData")) {
                let body = request_body(
                    &[(value, original)],
                    &strings(root.get("consumes")),
                    sources,
                    trace,
                )?;
                let map = output["components"].as_object_mut().ok_or_else(|| {
                    ConversionError::new("INTERNAL_ERROR", "Missing component map")
                })?;
                map.entry("requestBodies").or_insert_with(|| json!({}))[name] = body;
            } else {
                let parameter = parameter(
                    &value,
                    &original,
                    &child("/components/parameters", name),
                    sources,
                    trace,
                )?;
                let map = output["components"].as_object_mut().ok_or_else(|| {
                    ConversionError::new("INTERNAL_ERROR", "Missing component map")
                })?;
                map.entry("parameters").or_insert_with(|| json!({}))[name] = parameter;
            }
        }
    }
    if let Some(paths) = root.get("paths").and_then(Value::as_object) {
        for (path, item) in paths {
            if path.starts_with("x-") {
                output["paths"][path] = item.clone();
                continue;
            }
            let item_map = object(item, "Path item")?;
            let path_pointer = child("/paths", path);
            let inherited = parameters(
                item.get("parameters"),
                &child(&path_pointer, "parameters"),
                source,
            )?;
            let mut converted = Map::new();
            extensions(item_map, &mut converted);
            if let Some(reference) = item.get("$ref") {
                converted.insert("$ref".into(), reference.clone());
            }
            for (verb, operation) in item_map {
                if !method(verb) {
                    continue;
                }
                let at = child(&path_pointer, verb);
                converted.insert(
                    verb.clone(),
                    operation_to_openapi(operation, &at, &inherited, source, sources, trace)?,
                );
            }
            unknown_fields(
                item,
                &[
                    "$ref",
                    "parameters",
                    "get",
                    "put",
                    "post",
                    "delete",
                    "options",
                    "head",
                    "patch",
                    "trace",
                ],
                sources,
                uri,
                &path_pointer,
                trace,
            );
            output["paths"][path] = Value::Object(converted);
        }
    }
    if let Some(responses) = root.get("responses").and_then(Value::as_object) {
        let mut converted = Map::new();
        for (name, response) in responses {
            converted.insert(
                name.clone(),
                response_to_openapi(
                    response,
                    &child("/responses", name),
                    &child("/components/responses", name),
                    &strings(root.get("produces")),
                    sources,
                    trace,
                )?,
            );
        }
        output["components"]["responses"] = Value::Object(converted);
    }
    rewrite_refs(&mut output, source);
    unknown_fields(
        source,
        &[
            "swagger",
            "info",
            "host",
            "basePath",
            "schemes",
            "consumes",
            "produces",
            "paths",
            "definitions",
            "parameters",
            "responses",
            "securityDefinitions",
            "security",
            "tags",
            "externalDocs",
        ],
        sources,
        uri,
        "",
        trace,
    );
    trace.record(
        sources,
        uri,
        "/swagger",
        &["/openapi".into()],
        "SWAGGER_TO_OPENAPI",
        Classification::Transformed,
        "Swagger transport objects and schema semantics converted to OpenAPI",
        Some(&source["swagger"]),
    );
    Ok(output)
}

fn servers(source: &Value, sources: &Sources, config: &Config, trace: &mut Trace) -> Value {
    if let Some(url) = &config.base_url {
        return json!([{"url":url}]);
    }
    let base = source["basePath"].as_str().unwrap_or("/");
    let host = source["host"].as_str();
    let schemes = strings(source.get("schemes"));
    let origin = url::Url::parse(&sources.root)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"));
    let urls: Vec<_> = if let Some(host) = host {
        if schemes.is_empty() {
            vec![format!("//{host}{base}")]
        } else {
            schemes
                .iter()
                .map(|scheme| format!("{scheme}://{host}{base}"))
                .collect()
        }
    } else if let Some(origin) = origin {
        let host = origin.host_str().unwrap_or("");
        let authority = origin
            .port()
            .map_or_else(|| host.into(), |port| format!("{host}:{port}"));
        if schemes.is_empty() {
            vec![format!("{}://{authority}{base}", origin.scheme())]
        } else {
            schemes
                .iter()
                .map(|scheme| format!("{scheme}://{authority}{base}"))
                .collect()
        }
    } else {
        if !schemes.is_empty() {
            trace.record(sources, &sources.root, "/schemes", &["/servers".into()], "SERVER_HOST_UNKNOWN", Classification::Blocked, "Source declares schemes but no host and has no HTTP source origin. Set baseUrl explicitly.", source.get("schemes"));
        }
        vec![base.into()]
    };
    json!(
        urls.into_iter()
            .map(|url| json!({"url":url}))
            .collect::<Vec<_>>()
    )
}

fn parameters(
    raw: Option<&Value>,
    pointer: &str,
    document: &Value,
) -> Result<Vec<(Value, String)>> {
    raw.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, item)| resolve_parameter(item, &child(pointer, &index.to_string()), document))
        .collect()
}

fn resolve_parameter(value: &Value, pointer: &str, document: &Value) -> Result<(Value, String)> {
    let mut value = value;
    let mut pointer = pointer.to_owned();
    let mut seen = std::collections::BTreeSet::new();
    while let Some(reference) = value.get("$ref").and_then(Value::as_str) {
        let target = crate::source::local_pointer(reference)?;
        if !seen.insert(target.to_owned()) {
            return Err(ConversionError::new(
                "CYCLIC_PARAMETER",
                "Parameter reference cycle has no concrete parameter",
            ));
        }
        value = document.pointer(&target).ok_or_else(|| {
            ConversionError::new(
                "UNRESOLVED_REFERENCE",
                format!("Missing parameter target {reference}"),
            )
        })?;
        pointer = target;
    }
    Ok((value.clone(), pointer))
}

fn operation_to_openapi(
    operation: &Value,
    at: &str,
    inherited: &[(Value, String)],
    document: &Value,
    sources: &Sources,
    trace: &mut Trace,
) -> Result<Value> {
    let raw = object(operation, "Operation")?;
    let mut output = Map::new();
    for key in [
        "tags",
        "summary",
        "description",
        "externalDocs",
        "operationId",
        "deprecated",
        "security",
    ] {
        if let Some(value) = raw.get(key) {
            output.insert(key.into(), value.clone());
        }
    }
    extensions(raw, &mut output);
    let mut all = inherited.to_vec();
    for (parameter, location) in
        parameters(raw.get("parameters"), &child(at, "parameters"), document)?
    {
        all.retain(|(prior, _)| {
            prior["name"] != parameter["name"] || prior["in"] != parameter["in"]
        });
        all.push((parameter, location));
    }
    let mut regular = Vec::new();
    let mut payload = Vec::new();
    for (value, pointer) in all {
        if matches!(value["in"].as_str(), Some("body" | "formData")) {
            payload.push((value, pointer));
        } else {
            regular.push(parameter(
                &value,
                &pointer,
                &child(&child(at, "parameters"), &regular.len().to_string()),
                sources,
                trace,
            )?);
        }
    }
    if !regular.is_empty() {
        output.insert("parameters".into(), Value::Array(regular));
    }
    let consumes = strings(raw.get("consumes").or_else(|| document.get("consumes")));
    if !payload.is_empty() {
        output.insert(
            "requestBody".into(),
            request_body(&payload, &consumes, sources, trace)?,
        );
    }
    let produces = strings(raw.get("produces").or_else(|| document.get("produces")));
    let mut responses = Map::new();
    if let Some(raw) = operation.get("responses").and_then(Value::as_object) {
        for (status, response) in raw {
            responses.insert(
                status.clone(),
                response_to_openapi(
                    response,
                    &child(&child(at, "responses"), status),
                    &child(&child(at, "responses"), status),
                    &produces,
                    sources,
                    trace,
                )?,
            );
        }
    }
    output.insert("responses".into(), Value::Object(responses));
    unknown_fields(
        operation,
        &[
            "tags",
            "summary",
            "description",
            "externalDocs",
            "operationId",
            "deprecated",
            "security",
            "parameters",
            "responses",
            "consumes",
            "produces",
            "schemes",
        ],
        sources,
        &sources.root,
        at,
        trace,
    );
    if operation.get("schemes").is_some() {
        let mut server_source = document.clone();
        server_source["schemes"] = operation["schemes"].clone();
        output.insert(
            "servers".into(),
            servers(&server_source, sources, &Config::default(), trace),
        );
    }
    trace.record(
        sources,
        &sources.root,
        at,
        &[at.into()],
        "OPERATION",
        Classification::Transformed,
        "Operation preserves effective inherited parameters and explicit security overrides",
        Some(operation),
    );
    Ok(Value::Object(output))
}

fn parameter(
    value: &Value,
    from: &str,
    to: &str,
    sources: &Sources,
    trace: &mut Trace,
) -> Result<Value> {
    let raw = object(value, "Parameter")?;
    let mut output = Map::new();
    for key in PARAMETER_FIELDS {
        if let Some(value) = raw.get(*key) {
            output.insert((*key).into(), value.clone());
        }
    }
    extensions(raw, &mut output);
    let mut schema = Value::Object(
        raw.iter()
            .filter(|(key, _)| SCHEMA_FIELDS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    schema::to_modern(
        &mut schema,
        &SchemaSite {
            sources,
            uri: &sources.root,
            source: from,
            output: &child(to, "schema"),
        },
        true,
        trace,
    );
    output.insert("schema".into(), schema);
    serialization(value, &mut output, from, sources, trace);
    trace.record(
        sources,
        &sources.root,
        from,
        &[to.into()],
        "PARAMETER_SCHEMA",
        Classification::Transformed,
        "Parameter schema and serialization moved into OpenAPI fields",
        Some(value),
    );
    Ok(Value::Object(output))
}

fn serialization(
    value: &Value,
    output: &mut Map<String, Value>,
    pointer: &str,
    sources: &Sources,
    trace: &mut Trace,
) {
    if value["type"] != "array" {
        return;
    }
    let format = value["collectionFormat"].as_str().unwrap_or("csv");
    let location = value["in"].as_str().unwrap_or("");
    let style = match (location, format) {
        ("query" | "formData", "csv" | "multi") => Some("form"),
        ("query", "ssv") => Some("spaceDelimited"),
        ("query", "pipes") => Some("pipeDelimited"),
        ("path" | "header", "csv") => Some("simple"),
        _ => None,
    };
    if let Some(style) = style {
        output.insert("style".into(), json!(style));
        output.insert("explode".into(), json!(format == "multi"));
    } else {
        trace.record(sources,&sources.root,&child(pointer,"collectionFormat"),&[],"PARAMETER_SERIALIZATION",Classification::Blocked,format!("{format} serialization in {location} has no exact OpenAPI style; provide a reviewed encoding"),Some(value));
    }
}

fn request_body(
    parameters: &[(Value, String)],
    consumes: &[String],
    sources: &Sources,
    trace: &mut Trace,
) -> Result<Value> {
    let body: Vec<_> = parameters
        .iter()
        .filter(|(value, _)| value["in"] == "body")
        .collect();
    let form: Vec<_> = parameters
        .iter()
        .filter(|(value, _)| value["in"] == "formData")
        .collect();
    if body.len() > 1 || !body.is_empty() && !form.is_empty() {
        return Err(ConversionError::new(
            "CONFLICTING_BODY",
            "Swagger body and formData parameters cannot coexist",
        ));
    }
    let mut required = false;
    let mut description = None;
    let mut encoding = Map::new();
    let mut schema = if let Some((value, from)) = body.first() {
        required = value["required"] == true;
        description = value.get("description").cloned();
        let mut schema = value.get("schema").cloned().unwrap_or_else(|| json!({}));
        schema::to_modern(
            &mut schema,
            &SchemaSite {
                sources,
                uri: &sources.root,
                source: &child(from, "schema"),
                output: &child(from, "schema"),
            },
            true,
            trace,
        );
        schema
    } else {
        let mut properties = Map::new();
        let mut required_fields = Vec::new();
        for (value, from) in form {
            let name = value["name"].as_str().ok_or_else(|| {
                ConversionError::new("INVALID_PARAMETER", "Form parameter needs a name")
            })?;
            let mut field = Value::Object(
                object(value, "Form parameter")?
                    .iter()
                    .filter(|(key, _)| {
                        SCHEMA_FIELDS.contains(&key.as_str()) || key.as_str() == "description"
                    })
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            );
            schema::to_modern(
                &mut field,
                &SchemaSite {
                    sources,
                    uri: &sources.root,
                    source: from,
                    output: &child("/schema/properties", name),
                },
                true,
                trace,
            );
            properties.insert(name.into(), field);
            if value["required"] == true {
                required = true;
                required_fields.push(name);
            }
            let mut config = Map::new();
            serialization(value, &mut config, from, sources, trace);
            if !config.is_empty() {
                encoding.insert(name.into(), Value::Object(config));
            }
        }
        let mut schema = json!({"type":"object","properties":properties});
        if !required_fields.is_empty() {
            schema["required"] = json!(required_fields);
        }
        schema
    };
    let media = if consumes.is_empty() {
        trace.record(
            sources,
            &sources.root,
            &parameters[0].1,
            &[],
            "MEDIA_TYPE_INFERRED",
            Classification::Inferred,
            "No consumes media type was supplied; review the proposed request media type",
            Some(&parameters[0].0),
        );
        vec![if body.is_empty() {
            "application/x-www-form-urlencoded".into()
        } else {
            "application/json".into()
        }]
    } else {
        consumes.to_vec()
    };
    let mut content = Map::new();
    for media in media {
        let mut item = json!({"schema":schema.take()});
        schema = item["schema"].clone();
        if !encoding.is_empty() {
            item["encoding"] = Value::Object(encoding.clone());
        }
        content.insert(media, item);
    }
    let mut result = json!({"required":required,"content":content});
    if let Some(description) = description {
        result["description"] = description;
    }
    Ok(result)
}

fn response_to_openapi(
    value: &Value,
    from: &str,
    to: &str,
    produces: &[String],
    sources: &Sources,
    trace: &mut Trace,
) -> Result<Value> {
    if let Some(reference) = value.get("$ref") {
        return Ok(json!({"$ref":reference}));
    }
    let raw = object(value, "Response")?;
    let mut output = Map::new();
    extensions(raw, &mut output);
    if let Some(description) = raw.get("description") {
        output.insert("description".into(), description.clone());
    }
    if let Some(headers) = raw.get("headers").and_then(Value::as_object) {
        let mut converted = Map::new();
        for (name, header) in headers {
            let mut parameter_source = header.clone();
            parameter_source["in"] = json!("header");
            parameter_source["name"] = json!(name);
            let mut converted_header = parameter(
                &parameter_source,
                &child(&child(from, "headers"), name),
                &child(&child(to, "headers"), name),
                sources,
                trace,
            )?;
            if let Some(map) = converted_header.as_object_mut() {
                map.remove("in");
                map.remove("name");
            }
            converted.insert(name.clone(), converted_header);
        }
        output.insert("headers".into(), Value::Object(converted));
    }
    let examples = raw.get("examples").and_then(Value::as_object);
    if raw.get("schema").is_some() || examples.is_some() {
        let mut media = produces.to_vec();
        if let Some(examples) = examples {
            for name in examples.keys() {
                if !media.contains(name) {
                    media.push(name.clone());
                }
            }
        }
        if media.is_empty() {
            media.push("application/json".into());
            trace.record(
                sources,
                &sources.root,
                from,
                &[to.into()],
                "RESPONSE_MEDIA_INFERRED",
                Classification::Inferred,
                "No produces media type was declared; review the proposed response media type",
                Some(value),
            );
        }
        let mut content = Map::new();
        for media in media {
            let mut item = json!({});
            if let Some(schema) = raw.get("schema") {
                let mut schema = schema.clone();
                schema::to_modern(
                    &mut schema,
                    &SchemaSite {
                        sources,
                        uri: &sources.root,
                        source: &child(from, "schema"),
                        output: &child(&child(&child(to, "content"), &media), "schema"),
                    },
                    true,
                    trace,
                );
                item["schema"] = schema;
            }
            if let Some(example) = examples.and_then(|examples| examples.get(&media)) {
                item["example"] = example.clone();
            }
            content.insert(media, item);
        }
        output.insert("content".into(), Value::Object(content));
    }
    unknown_fields(
        value,
        &["description", "schema", "headers", "examples"],
        sources,
        &sources.root,
        from,
        trace,
    );
    trace.record(
        sources,
        &sources.root,
        from,
        &[to.into()],
        "RESPONSE_CONTENT",
        Classification::Transformed,
        "Response media types, schema, headers and examples retained",
        Some(value),
    );
    Ok(Value::Object(output))
}

fn security_scheme(value: &Value, sources: &Sources, at: &str, trace: &mut Trace) -> Value {
    let mut output = value.clone();
    if value["type"] == "basic" {
        output["type"] = json!("http");
        output["scheme"] = json!("basic");
    }
    if value["type"] == "oauth2" {
        let flow = match value["flow"].as_str() {
            Some("implicit") => "implicit",
            Some("password") => "password",
            Some("application") => "clientCredentials",
            Some("accessCode") => "authorizationCode",
            _ => "unknown",
        };
        let mut settings = Map::new();
        for key in ["authorizationUrl", "tokenUrl", "scopes"] {
            if let Some(value) = value.get(key) {
                settings.insert(key.into(), value.clone());
            }
        }
        if let Some(map) = output.as_object_mut() {
            for key in ["flow", "authorizationUrl", "tokenUrl", "scopes"] {
                map.remove(key);
            }
            map.insert("flows".into(), json!({flow:settings}));
        }
    }
    trace.record(
        sources,
        &sources.root,
        at,
        &[at.replace("/securityDefinitions/", "/components/securitySchemes/")],
        "SECURITY_SCHEME",
        Classification::Transformed,
        "Authentication scheme mapped without weakening requirement AND/OR semantics",
        Some(value),
    );
    output
}

fn rewrite_refs(value: &mut Value, source: &Value) {
    crate::api::structural(value, "", &mut |object, _, kind| {
        if matches!(
            kind,
            crate::api::Kind::PathItem
                | crate::api::Kind::Parameter
                | crate::api::Kind::Response
                | crate::api::Kind::Header
                | crate::api::Kind::RequestBody
                | crate::api::Kind::SecurityScheme
        ) {
            rewrite_reference(object, source);
        }
    });
    crate::api::schemas(value, &mut |schema, at| {
        crate::schema::walk(schema, at, &mut |schema, _| {
            rewrite_reference(schema, source)
        });
    });
}

fn rewrite_reference(value: &mut Value, source: &Value) {
    let Some(reference) = value.get("$ref").and_then(Value::as_str) else {
        return;
    };
    let renamed = if let Some(suffix) = reference.strip_prefix("#/definitions/") {
        format!("#/components/schemas/{suffix}")
    } else if let Some(suffix) = reference.strip_prefix("#/responses/") {
        format!("#/components/responses/{suffix}")
    } else if let Some(suffix) = reference.strip_prefix("#/parameters/") {
        let location = source
            .pointer(reference.trim_start_matches('#'))
            .and_then(|value| value.get("in"))
            .and_then(Value::as_str);
        format!(
            "#/components/{}/{suffix}",
            if matches!(location, Some("body" | "formData")) {
                "requestBodies"
            } else {
                "parameters"
            }
        )
    } else {
        return;
    };
    value["$ref"] = json!(renamed);
}
