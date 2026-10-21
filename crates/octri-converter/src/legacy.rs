//! OpenAPI 3.x transport -> Swagger 2.0. Unrepresentable semantics block export.

use crate::{Classification, Config, ConversionError, Result};
use crate::{
    emit::record_incompatible,
    source::{Sources, child},
    trace::Trace,
};
use serde_json::{Map, Value, json};

struct Context<'a> {
    sources: &'a Sources,
    config: &'a Config,
    original: &'a Value,
    trace: &'a mut Trace,
}

pub(crate) fn emit(
    document: &mut Value,
    sources: &Sources,
    config: &Config,
    trace: &mut Trace,
) -> Result<()> {
    let original = document.clone();
    let mut context = Context {
        sources,
        config,
        original: &original,
        trace,
    };
    let mut result = json!({"swagger":"2.0","info":original["info"],"paths":{}});
    let mut response_output_bytes = 0usize;
    let mut response_output_nodes = 0usize;
    for key in ["tags", "externalDocs", "security"] {
        if let Some(value) = original.get(key) {
            result[key] = value.clone();
        }
    }
    if let Some(map) = original.as_object() {
        for (key, value) in map {
            if key.starts_with("x-") {
                result[key] = value.clone();
            }
        }
    }
    let server = context.servers(original.get("servers"), "/servers");
    for (key, value) in server.as_object().into_iter().flatten() {
        result[key] = value.clone();
    }
    if let Some(components) = original.get("components").and_then(Value::as_object) {
        if let Some(schemas) = components.get("schemas") {
            result["definitions"] = schemas.clone();
        }
        if let Some(parameters) = components.get("parameters").and_then(Value::as_object) {
            let mut converted = Map::new();
            for (name, value) in parameters {
                converted.insert(
                    name.clone(),
                    context.parameter(value, &child("/components/parameters", name)),
                );
            }
            result["parameters"] = Value::Object(converted);
        }
        if let Some(responses) = components.get("responses").and_then(Value::as_object) {
            let mut converted = Map::new();
            for (name, value) in responses {
                converted.insert(
                    name.clone(),
                    context
                        .response(value, &child("/components/responses", name))
                        .0,
                );
            }
            result["responses"] = Value::Object(converted);
        }
        if let Some(security) = components.get("securitySchemes").and_then(Value::as_object) {
            let mut converted = Map::new();
            for (name, value) in security {
                converted.insert(
                    name.clone(),
                    context.security(value, &child("/components/securitySchemes", name)),
                );
            }
            result["securityDefinitions"] = Value::Object(converted);
        }
        for key in components.keys().filter(|key| {
            !matches!(
                key.as_str(),
                "schemas" | "parameters" | "responses" | "securitySchemes" | "requestBodies"
            ) && !key.starts_with("x-")
        }) {
            context.block(&child("/components", key), &components[key]);
        }
    }
    if let Some(paths) = original.get("paths").and_then(Value::as_object) {
        for (path, item) in paths {
            let at = child("/paths", path);
            if path.starts_with("x-") {
                result["paths"][path] = item.clone();
                continue;
            }
            let mut out = json!({});
            if let Some(parameters) = item.get("parameters").and_then(Value::as_array) {
                out["parameters"] = Value::Array(
                    parameters
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            context.parameter(
                                value,
                                &child(&child(&at, "parameters"), &index.to_string()),
                            )
                        })
                        .collect(),
                );
            }
            if let Some(reference) = item.get("$ref") {
                out["$ref"] = reference.clone();
            }
            for key in ["summary", "description", "servers"] {
                if let Some(value) = item.get(key) {
                    context.block(&child(&at, key), value);
                }
            }
            for (verb, operation) in item.as_object().into_iter().flatten() {
                if verb.starts_with("x-") {
                    out[verb] = operation.clone();
                    continue;
                }
                if !super::adapters::common::method(verb) {
                    continue;
                }
                let op_at = child(&at, verb);
                if verb == "query" {
                    context.block(&op_at, operation);
                    continue;
                }
                let mut op = operation.clone();
                if let Some(map) = op.as_object_mut() {
                    for key in ["callbacks", "servers"] {
                        if let Some(value) = map.remove(key) {
                            if key == "servers" {
                                let local = context.servers(Some(&value), &child(&op_at, key));
                                if local.get("host") != server.get("host")
                                    || local.get("basePath") != server.get("basePath")
                                {
                                    context.block(&child(&op_at, key), &value);
                                }
                                if let Some(schemes) = local.get("schemes") {
                                    map.insert("schemes".into(), schemes.clone());
                                }
                            } else {
                                context.block(&child(&op_at, key), &value);
                            }
                        }
                    }
                    let mut parameters = map
                        .remove("parameters")
                        .and_then(|value| value.as_array().cloned())
                        .unwrap_or_default()
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            context.parameter(
                                value,
                                &child(&child(&op_at, "parameters"), &index.to_string()),
                            )
                        })
                        .collect::<Vec<_>>();
                    if let Some(body) = map.remove("requestBody") {
                        let body = context.resolve(&body)?.clone();
                        let (mut payload, consumes) =
                            context.body(&body, &child(&op_at, "requestBody"));
                        parameters.append(&mut payload);
                        if !consumes.is_empty() {
                            map.insert("consumes".into(), json!(consumes));
                        }
                    }
                    if !parameters.is_empty() {
                        map.insert("parameters".into(), Value::Array(parameters));
                    }
                    if let Some(responses) = map.get_mut("responses").and_then(Value::as_object_mut)
                    {
                        let mut produces = std::collections::BTreeSet::new();
                        let explicit: std::collections::BTreeSet<_> = responses
                            .keys()
                            .filter(|status| !status.ends_with("XX"))
                            .cloned()
                            .collect();
                        let mut expanded = Map::new();
                        for (status, response) in std::mem::take(responses) {
                            let pointer = child(&child(&op_at, "responses"), &status);
                            let (converted, media) = context.response(&response, &pointer);
                            let statuses = if status.ends_with("XX") {
                                let first = status
                                    .as_bytes()
                                    .first()
                                    .copied()
                                    .filter(|first| matches!(first, b'1'..=b'5'))
                                    .ok_or_else(|| {
                                        crate::ConversionError::new(
                                            "RESPONSE_STATUS",
                                            "Invalid HTTP response range",
                                        )
                                    })?;
                                let codes = (0..100)
                                    .map(|suffix| format!("{}{suffix:02}", char::from(first)))
                                    .filter(|code| !explicit.contains(code))
                                    .collect::<Vec<_>>();
                                context.trace.record(context.sources,&context.sources.root,&pointer,&codes.iter().map(|code|child(&child(&op_at,"responses"),code)).collect::<Vec<_>>(),"HTTP_STATUS_RANGE_EXPANDED",Classification::Transformed,"Swagger 2.0 has no response ranges. Expanded range into exact status codes; explicit responses retain precedence.",Some(&json!(status)));
                                codes
                            } else {
                                vec![status]
                            };
                            let bytes = serde_json::to_vec(&converted)
                                .map_err(|error| {
                                    crate::ConversionError::new(
                                        "OUTPUT_ENCODING",
                                        error.to_string(),
                                    )
                                })?
                                .len();
                            response_output_bytes = response_output_bytes
                                .saturating_add(bytes.saturating_mul(statuses.len()));
                            let mut queue = vec![&converted];
                            let mut nodes = 0usize;
                            while let Some(value) = queue.pop() {
                                nodes += 1;
                                match value {
                                    Value::Object(map) => queue.extend(map.values()),
                                    Value::Array(values) => queue.extend(values),
                                    _ => {}
                                }
                            }
                            response_output_nodes = response_output_nodes
                                .saturating_add(nodes.saturating_mul(statuses.len()));
                            if response_output_bytes > config.limits.max_output_bytes
                                || response_output_nodes > config.limits.max_nodes
                            {
                                return Err(crate::ConversionError::new(
                                    "LIMIT_RESPONSE_EXPANSION",
                                    "Expanded response ranges exceed output byte or node limit",
                                ));
                            }
                            for status in statuses {
                                expanded.insert(status, converted.clone());
                            }
                            produces.extend(media);
                        }
                        *responses = expanded;
                        if !produces.is_empty() {
                            map.insert("produces".into(), json!(produces));
                        }
                    }
                }
                out[verb] = op;
            }
            result["paths"][path] = out;
        }
    }
    rewrite_refs(&mut result);
    *document = result;
    Ok(())
}

impl Context<'_> {
    fn block(&mut self, pointer: &str, value: &Value) {
        record_incompatible(pointer, value, "2.0", self.sources, self.config, self.trace);
    }

    fn resolve<'a>(&'a self, value: &'a Value) -> Result<&'a Value> {
        let mut current = value;
        let mut seen = std::collections::BTreeSet::new();
        while let Some(reference) = current.get("$ref").and_then(Value::as_str) {
            if !seen.insert(reference) {
                return Err(ConversionError::new(
                    "CYCLIC_REFERENCE",
                    "Reference cycle has no concrete transport object",
                ));
            }
            let pointer = crate::source::local_pointer(reference)?;
            current = self.original.pointer(&pointer).ok_or_else(|| {
                ConversionError::new(
                    "UNRESOLVED_REFERENCE",
                    format!("Missing target {reference}"),
                )
            })?;
        }
        Ok(current)
    }

    fn servers(&mut self, value: Option<&Value>, pointer: &str) -> Value {
        let mut urls = Vec::new();
        if let Some(override_url) = &self.config.base_url {
            urls.push(override_url.clone());
        } else {
            for (index, server) in value
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                let mut url = server["url"].as_str().unwrap_or("/").to_owned();
                for (name, variable) in server
                    .get("variables")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    let default = variable["default"].as_str().unwrap_or("");
                    url = url.replace(&format!("{{{name}}}"), default);
                    if variable
                        .get("enum")
                        .and_then(Value::as_array)
                        .is_none_or(|values| values.len() != 1)
                    {
                        self.block(
                            &child(
                                &child(&child(pointer, &index.to_string()), "variables"),
                                name,
                            ),
                            variable,
                        );
                    }
                }
                urls.push(url);
            }
        }
        if urls.is_empty() {
            urls.push("/".into());
        }
        let mut result = json!({});
        let mut schemes = std::collections::BTreeSet::new();
        for (index, url) in urls.iter().enumerate() {
            let parsed = url::Url::parse(url)
                .ok()
                .filter(|url| matches!(url.scheme(), "http" | "https" | "ws" | "wss"));
            let mut item = json!({"basePath":url});
            if let Some(url) = parsed {
                let host = url.host_str().unwrap_or("");
                let host = if host.contains(':') {
                    format!("[{host}]")
                } else {
                    host.into()
                };
                item = json!({"host":url.port().map_or_else(||host.clone(),|port|format!("{host}:{port}")),"basePath":url.path()});
                schemes.insert(url.scheme().to_owned());
                if url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() {
                    self.block(pointer, value.unwrap_or(&Value::Null));
                }
            } else if !url.starts_with('/') || url.contains(['{', '?', '#']) {
                self.block(pointer, value.unwrap_or(&Value::Null));
            }
            if index == 0 {
                result = item;
            } else if result != item {
                self.block(pointer, value.unwrap_or(&Value::Null));
            }
        }
        if !schemes.is_empty() {
            result["schemes"] = json!(schemes);
        }
        result
    }

    fn parameter(&mut self, value: &Value, pointer: &str) -> Value {
        if value.get("$ref").is_some() {
            return value.clone();
        }
        let mut result = value.clone();
        if value["in"] == "cookie" || value["in"] == "querystring" {
            self.block(&child(pointer, "in"), &value["in"]);
        }
        if let Some(schema) = value.get("schema") {
            if schema.get("$ref").is_some() || schema["type"] == "object" {
                self.block(&child(pointer, "schema"), schema);
            }
            if let Some(map) = result.as_object_mut() {
                map.remove("schema");
                for (key, value) in schema.as_object().into_iter().flatten() {
                    map.insert(key.clone(), value.clone());
                }
            }
        }
        if let Some(content) = value.get("content") {
            self.block(&child(pointer, "content"), content);
        }
        for key in [
            "example",
            "examples",
            "deprecated",
            "allowReserved",
            "content",
        ] {
            if let Some(value) = result.as_object_mut().and_then(|map| map.remove(key)) {
                self.block(&child(pointer, key), &value);
            }
        }
        let style = value["style"].as_str().unwrap_or(
            if value["in"] == "query" || value["in"] == "cookie" {
                "form"
            } else {
                "simple"
            },
        );
        let explode = value["explode"].as_bool().unwrap_or(style == "form");
        if result["type"] == "array" {
            let format = match (style, explode) {
                ("form", true) => Some("multi"),
                ("form" | "simple", false) => Some("csv"),
                ("spaceDelimited", false) => Some("ssv"),
                ("pipeDelimited", false) => Some("pipes"),
                _ => None,
            };
            if let Some(format) = format {
                result["collectionFormat"] = json!(format);
            } else {
                self.block(pointer, value);
            }
        } else if !matches!(style, "form" | "simple") || explode && style == "simple" {
            self.block(pointer, value);
        }
        if let Some(map) = result.as_object_mut() {
            map.remove("style");
            map.remove("explode");
        }
        result
    }

    fn content_schema(&mut self, content: &Value, pointer: &str) -> (Value, Vec<String>) {
        let mut schema = None;
        let mut media = Vec::new();
        for (name, item) in content.as_object().into_iter().flatten() {
            media.push(name.clone());
            let candidate = item.get("schema").cloned().unwrap_or_else(|| json!({}));
            if let Some(first) = &schema {
                if first != &candidate {
                    self.block(&child(pointer, name), item);
                }
            } else {
                schema = Some(candidate);
            }
            for key in ["encoding", "examples", "example"] {
                if let Some(value) = item.get(key) {
                    self.block(&child(&child(pointer, name), key), value);
                }
            }
        }
        (schema.unwrap_or_else(|| json!({})), media)
    }

    fn body(&mut self, value: &Value, pointer: &str) -> (Vec<Value>, Vec<String>) {
        let (schema, media) = self.content_schema(&value["content"], &child(pointer, "content"));
        let extensions = self.content_extensions(&value["content"], &child(pointer, "content"));
        let is_form = !media.is_empty()
            && media.iter().all(|name| {
                matches!(
                    name.as_str(),
                    "application/x-www-form-urlencoded" | "multipart/form-data"
                )
            });
        if is_form {
            let mut parameters = Vec::new();
            for (name, field) in schema
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let mut parameter = field.clone();
                if parameter["type"] == "string" && parameter["format"] == "binary" {
                    parameter["type"] = json!("file");
                    if let Some(map) = parameter.as_object_mut() {
                        map.remove("format");
                    }
                }
                if parameter["type"] == "object" || parameter.get("$ref").is_some() {
                    self.block(pointer, field);
                }
                parameter["in"] = json!("formData");
                parameter["name"] = json!(name);
                parameter["required"] = json!(
                    value["required"] == true
                        && schema
                            .get("required")
                            .and_then(Value::as_array)
                            .is_some_and(|names| names.contains(&json!(name)))
                );
                parameters.push(parameter);
            }
            if schema.get("additionalProperties").is_some()
                || schema.get("allOf").is_some()
                || parameters.is_empty()
            {
                self.block(pointer, value);
            }
            if let (Some(extensions), Some(parameter)) = (extensions, parameters.first_mut()) {
                parameter["x-octri-content-extensions"] = extensions;
            }
            (parameters, media)
        } else {
            let mut parameter = json!({"in":"body","name":"body","schema":schema});
            if let Some(extensions) = extensions {
                parameter["x-octri-content-extensions"] = extensions;
            }
            for key in ["description", "required"] {
                if let Some(value) = value.get(key) {
                    parameter[key] = value.clone();
                }
            }
            (vec![parameter], media)
        }
    }

    fn response(&mut self, value: &Value, pointer: &str) -> (Value, Vec<String>) {
        if value.get("$ref").is_some() {
            return (value.clone(), Vec::new());
        }
        let mut result = value.clone();
        let mut media = Vec::new();
        if let Some(content) = result.as_object_mut().and_then(|map| map.remove("content")) {
            if let Some(extensions) = self.content_extensions(&content, &child(pointer, "content"))
            {
                result["x-octri-content-extensions"] = extensions;
            }
            let (schema, types) = self.content_schema(&content, &child(pointer, "content"));
            media = types;
            result["schema"] = schema;
            let mut examples = Map::new();
            for (name, item) in content.as_object().into_iter().flatten() {
                if let Some(example) = item.get("example") {
                    examples.insert(name.clone(), example.clone());
                }
            }
            if !examples.is_empty() {
                result["examples"] = Value::Object(examples);
            }
        }
        if let Some(links) = result.as_object_mut().and_then(|map| map.remove("links")) {
            self.block(&child(pointer, "links"), &links);
        }
        if let Some(headers) = result.get_mut("headers").and_then(Value::as_object_mut) {
            for (name, value) in headers {
                let mut header = self.parameter(value, &child(&child(pointer, "headers"), name));
                if let Some(map) = header.as_object_mut() {
                    map.remove("required");
                }
                *value = header;
            }
        }
        (result, media)
    }

    fn content_extensions(&mut self, content: &Value, pointer: &str) -> Option<Value> {
        let mut retained = Map::new();
        for (media, item) in content.as_object().into_iter().flatten() {
            let extensions = item
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(key, _)| key.starts_with("x-"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Map<_, _>>();
            if !extensions.is_empty() {
                let extensions = Value::Object(extensions);
                self.trace.record(self.sources,&self.sources.root,&child(pointer,media),&[],"LEGACY_MEDIA_EXTENSIONS",Classification::Omitted,"Swagger 2.0 has no Media Type Object. Its extensions remain as inert original-context evidence in x-octri-content-extensions; consumer behavior needs review.",Some(&extensions));
                retained.insert(media.clone(), extensions);
            }
        }
        if retained.is_empty() {
            None
        } else {
            Some(Value::Object(retained))
        }
    }

    fn security(&mut self, value: &Value, pointer: &str) -> Value {
        let mut result = value.clone();
        match value["type"].as_str() {
            Some("http")
                if value["scheme"]
                    .as_str()
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case("basic")) =>
            {
                result["type"] = json!("basic");
                if let Some(map) = result.as_object_mut() {
                    map.remove("scheme");
                }
            }
            Some("apiKey") if matches!(value["in"].as_str(), Some("query" | "header")) => {}
            Some("oauth2") => {
                let flows = value.get("flows").and_then(Value::as_object);
                if flows.is_none_or(|flows| flows.len() != 1) {
                    self.block(&child(pointer, "flows"), value);
                }
                if let Some((name, settings)) = flows.into_iter().flatten().next() {
                    let flow = match name.as_str() {
                        "implicit" => "implicit",
                        "password" => "password",
                        "clientCredentials" => "application",
                        "authorizationCode" => "accessCode",
                        _ => "unknown",
                    };
                    result["flow"] = json!(flow);
                    for key in ["authorizationUrl", "tokenUrl", "scopes"] {
                        if let Some(value) = settings.get(key) {
                            result[key] = value.clone();
                        }
                    }
                    if let Some(value) = settings.get("refreshUrl") {
                        self.block(&child(pointer, "flows"), value);
                    }
                }
                if let Some(map) = result.as_object_mut() {
                    map.remove("flows");
                }
            }
            _ => self.block(pointer, value),
        }
        result
    }
}

fn rewrite_refs(value: &mut Value) {
    // Names in schema/parameter/response maps are user data. A property named
    // `example` or `x-record` is still a schema; literal examples stay inert.
    crate::api::structural(value, "", &mut |object, _, kind| {
        if matches!(
            kind,
            crate::api::Kind::PathItem
                | crate::api::Kind::Parameter
                | crate::api::Kind::Response
                | crate::api::Kind::Header
        ) {
            rewrite_reference(object);
            rewrite_schema_field(object);
        }
    });
    for (name, schema) in value
        .get_mut("definitions")
        .and_then(Value::as_object_mut)
        .into_iter()
        .flatten()
    {
        crate::schema::walk(schema, &child("/definitions", name), &mut |schema, _| {
            rewrite_reference(schema)
        });
    }
    for parameter in value
        .get_mut("parameters")
        .and_then(Value::as_object_mut)
        .into_iter()
        .flat_map(|map| map.values_mut())
    {
        rewrite_reference(parameter);
        rewrite_schema_field(parameter);
    }
    for response in value
        .get_mut("responses")
        .and_then(Value::as_object_mut)
        .into_iter()
        .flat_map(|map| map.values_mut())
    {
        rewrite_reference(response);
        rewrite_schema_field(response);
    }
}

fn rewrite_schema_field(value: &mut Value) {
    if let Some(schema) = value.get_mut("schema") {
        crate::schema::walk(schema, "", &mut |schema, _| rewrite_reference(schema));
    }
}

fn rewrite_reference(value: &mut Value) {
    let Some(reference) = value.get("$ref").and_then(Value::as_str) else {
        return;
    };
    for (old, new) in [
        ("#/components/schemas/", "#/definitions/"),
        ("#/components/parameters/", "#/parameters/"),
        ("#/components/responses/", "#/responses/"),
    ] {
        if let Some(suffix) = reference.strip_prefix(old) {
            value["$ref"] = json!(format!("{new}{suffix}"));
            break;
        }
    }
}
