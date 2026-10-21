//! Postman collections. Scripts remain inert; variable precedence is explicit.

use super::{
    common::{object, unknown_fields},
    http::{self, Auth, Header, Observation, Payload, Response},
    references::Schemas,
};
use crate::{Classification, Config, ConversionError, Result};
use crate::{
    source::{Sources, child},
    trace::Trace,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

mod v3;

struct Context<'a> {
    sources: &'a mut Sources,
    config: &'a Config,
    trace: &'a mut Trace,
    variables: BTreeMap<String, String>,
    priority_variables: BTreeMap<String, String>,
    requests: Vec<Observation>,
    schemas: Schemas,
}

pub(crate) fn convert(sources: &mut Sources, config: &Config, trace: &mut Trace) -> Result<Value> {
    let uri = sources.root.clone();
    let document = sources.parse(&uri, false)?;
    if document["$kind"] == "collection" {
        if let Some(name) = &config.environment_uri {
            let environment_uri = sources.relative(&uri, name)?;
            sources.parse(&environment_uri, false)?;
        }
        return v3::convert(&document, sources, config, trace);
    }
    let declaration = document
        .pointer("/info/schema")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !["/v2.0.0/", "/v2.1.0/"]
        .iter()
        .any(|version| declaration.contains(version))
    {
        return Err(ConversionError::new(
            "POSTMAN_VERSION",
            "Supply a Postman Collection 2.0 or 2.1 export",
        ));
    }
    let mut variables = variables(document.get("variable"))?;
    let mut priority_variables = BTreeMap::new();
    if let Some(name) = &config.environment_uri {
        let env_uri = sources.relative(&uri, name)?;
        let environment = sources.parse(&env_uri, false)?;
        priority_variables.extend(variables_from(environment.get("values"), "key")?);
    }
    priority_variables.extend(config.variables.clone());
    variables.extend(priority_variables.clone());
    let title = document
        .pointer("/info/name")
        .and_then(Value::as_str)
        .unwrap_or("Converted Postman collection");
    let mut context = Context {
        sources,
        config,
        trace,
        variables,
        priority_variables,
        requests: Vec::new(),
        schemas: Schemas::default(),
    };
    let auth = context.auth(document.get("auth"), "/auth")?;
    context.events(&document, "");
    context.items(document.get("item"), "/item", &[], &auth, 0)?;
    unknown_fields(
        &document,
        &[
            "info",
            "item",
            "auth",
            "event",
            "variable",
            "protocolProfileBehavior",
        ],
        context.sources,
        &uri,
        "",
        context.trace,
    );
    if let Some(behavior) = document.get("protocolProfileBehavior") {
        context.record(
            "/protocolProfileBehavior",
            "POSTMAN_RUNTIME_SETTINGS",
            Classification::Omitted,
            "Postman runtime settings do not define an OpenAPI contract",
            Some(behavior),
        );
    }
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
    Ok(output)
}

fn variables(value: Option<&Value>) -> Result<BTreeMap<String, String>> {
    variables_from(value, "key")
}

fn variables_from(value: Option<&Value>, key: &str) -> Result<BTreeMap<String, String>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let entries = value.as_array().ok_or_else(|| {
        ConversionError::new("POSTMAN_VARIABLE", "Variable list must be an array")
    })?;
    let mut result = BTreeMap::new();
    for variable in entries {
        if variable["disabled"] == true || variable["enabled"] == false {
            continue;
        }
        let name = variable[key].as_str().ok_or_else(|| {
            ConversionError::new("POSTMAN_VARIABLE", "Variable needs a string key")
        })?;
        let value = match variable.get("value") {
            Some(Value::String(value)) => value.clone(),
            Some(value) if !value.is_null() => value.to_string(),
            _ => String::new(),
        };
        if result.insert(name.into(), value).is_some() {
            return Err(ConversionError::new(
                "POSTMAN_VARIABLE",
                format!("Duplicate active variable {name}"),
            ));
        }
    }
    Ok(result)
}

impl Context<'_> {
    fn items(
        &mut self,
        value: Option<&Value>,
        pointer: &str,
        tags: &[String],
        inherited_auth: &Auth,
        depth: usize,
    ) -> Result<()> {
        if depth > self.config.limits.max_depth {
            return Err(ConversionError::new(
                "POSTMAN_DEPTH",
                "Collection folder nesting exceeds configured limit",
            ));
        }
        let entries = value.and_then(Value::as_array).ok_or_else(|| {
            ConversionError::new("POSTMAN_ITEMS", format!("Expected item array at {pointer}"))
        })?;
        for (index, item) in entries.iter().enumerate() {
            let at = child(pointer, &index.to_string());
            object(item, "Postman item")?;
            let mut local_tags = tags.to_vec();
            // Null means inherit in a Postman export. Only noauth removes auth.
            let auth = if let Some(auth) = item.get("auth").filter(|auth| !auth.is_null()) {
                self.auth(Some(auth), &child(&at, "auth"))?
            } else {
                inherited_auth.clone()
            };
            self.events(item, &at);
            if let Some(folder_variables) = item.get("variable") {
                // Folder variables are local to their children. Config overrides
                // remain highest priority, regardless of nesting depth.
                let previous = self.variables.clone();
                self.variables.extend(variables(Some(folder_variables))?);
                self.variables.extend(self.priority_variables.clone());
                if let Some(name) = item["name"].as_str() {
                    local_tags.push(name.into());
                }
                self.items(
                    item.get("item"),
                    &child(&at, "item"),
                    &local_tags,
                    &auth,
                    depth + 1,
                )?;
                self.variables = previous;
            } else if item.get("item").is_some() {
                if let Some(name) = item["name"].as_str() {
                    local_tags.push(name.into());
                }
                self.items(
                    item.get("item"),
                    &child(&at, "item"),
                    &local_tags,
                    &auth,
                    depth + 1,
                )?;
            } else if let Some(request) = item.get("request") {
                let request = self.request(item, request, &at, tags, &auth)?;
                self.requests.push(request);
                if self.requests.len() > self.config.limits.max_nodes {
                    return Err(ConversionError::new(
                        "LIMIT_OPERATIONS",
                        "Too many requests",
                    ));
                }
            } else {
                return Err(ConversionError::new(
                    "POSTMAN_ITEM",
                    format!("Item at {at} has neither request nor folder items"),
                ));
            }
            unknown_fields(
                item,
                &[
                    "name",
                    "id",
                    "description",
                    "item",
                    "request",
                    "response",
                    "auth",
                    "event",
                    "variable",
                    "protocolProfileBehavior",
                ],
                self.sources,
                &self.sources.root,
                &at,
                self.trace,
            );
            if let Some(behavior) = item.get("protocolProfileBehavior") {
                self.record(
                    &child(&at, "protocolProfileBehavior"),
                    "POSTMAN_RUNTIME_SETTINGS",
                    Classification::Omitted,
                    "Runtime settings retained in source; no execution occurs",
                    Some(behavior),
                );
            }
        }
        Ok(())
    }

    fn request(
        &mut self,
        item: &Value,
        request: &Value,
        pointer: &str,
        tags: &[String],
        inherited_auth: &Auth,
    ) -> Result<Observation> {
        let at = child(pointer, "request");
        let (method, url) = if let Some(url) = request.as_str() {
            ("GET".into(), self.expand(url, &at))
        } else {
            object(request, "Postman request")?;
            (
                request["method"]
                    .as_str()
                    .ok_or_else(|| {
                        ConversionError::new(
                            "POSTMAN_METHOD",
                            format!("Missing HTTP method at {at}"),
                        )
                    })?
                    .into(),
                self.url(&request["url"], &child(&at, "url"))?,
            )
        };
        let headers = self.headers(request.get("header"), &child(&at, "header"))?;
        let auth = if let Some(auth) = request.get("auth").filter(|auth| !auth.is_null()) {
            self.auth(Some(auth), &child(&at, "auth"))?
        } else {
            inherited_auth.clone()
        };
        let body = self.body(request.get("body"), &headers, &child(&at, "body"))?;
        let mut path_variables = BTreeMap::new();
        for variable in request
            .pointer("/url/variable")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(name) = variable["key"].as_str() {
                path_variables.insert(
                    name.into(),
                    self.expand(
                        variable["value"].as_str().unwrap_or(""),
                        &child(&at, "url/variable"),
                    ),
                );
            }
        }
        let mut responses = Vec::new();
        if let Some(raw) = item.get("response") {
            for (index, response) in raw
                .as_array()
                .ok_or_else(|| {
                    ConversionError::new("POSTMAN_RESPONSE", "Response examples must be an array")
                })?
                .iter()
                .enumerate()
            {
                let location = child(&child(pointer, "response"), &index.to_string());
                let code = response["code"]
                    .as_u64()
                    .filter(|code| (100..=599).contains(code))
                    .ok_or_else(|| {
                        ConversionError::new(
                            "POSTMAN_RESPONSE",
                            format!("Response needs an HTTP status code at {location}"),
                        )
                    })?;
                let headers = self.headers(response.get("header"), &child(&location, "header"))?;
                let media = headers
                    .iter()
                    .find(|header| header.name.eq_ignore_ascii_case("content-type"))
                    .map(|header| header.value.as_str());
                let text = response["body"].as_str().unwrap_or("");
                let payload =
                    http::example_payload(text, media, &child(&location, "body"), self.sources)?;
                responses.push(Response {
                    status: code.to_string(),
                    description: response["name"]
                        .as_str()
                        .or_else(|| response["status"].as_str())
                        .unwrap_or("Observed response")
                        .into(),
                    headers,
                    payload,
                    pointer: location.clone(),
                });
                unknown_fields(
                    response,
                    &[
                        "id",
                        "name",
                        "originalRequest",
                        "status",
                        "code",
                        "_postman_previewlanguage",
                        "header",
                        "cookie",
                        "body",
                        "responseTime",
                    ],
                    self.sources,
                    &self.sources.root,
                    &location,
                    self.trace,
                );
                if let Some(cookies) = response
                    .get("cookie")
                    .filter(|value| value.as_array().is_some_and(|values| !values.is_empty()))
                {
                    self.record(&child(&location,"cookie"),"RESPONSE_COOKIES",Classification::Omitted,"Response cookies retained as observations; authored Set-Cookie contract required",Some(cookies));
                }
            }
        }
        unknown_fields(
            request,
            &[
                "method",
                "header",
                "body",
                "url",
                "auth",
                "description",
                "proxy",
                "certificate",
            ],
            self.sources,
            &self.sources.root,
            &at,
            self.trace,
        );
        for key in ["proxy", "certificate"] {
            if let Some(value) = request.get(key) {
                self.record(
                    &child(&at, key),
                    "POSTMAN_TRANSPORT_SETTINGS",
                    Classification::Omitted,
                    "Client transport configuration retained in source; it is not an API contract",
                    Some(value),
                );
            }
        }
        Ok(Observation {
            pointer: pointer.into(),
            name: item["name"].as_str().unwrap_or("Imported request").into(),
            description: description(
                request
                    .get("description")
                    .or_else(|| item.get("description")),
            ),
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

    fn url(&mut self, value: &Value, pointer: &str) -> Result<String> {
        if let Some(raw) = value.as_str() {
            return Ok(self.expand(raw, pointer));
        }
        object(value, "Postman URL")?;
        let mut raw = if let Some(raw) = value["raw"].as_str() {
            self.expand(raw, pointer)
        } else {
            let host = value["host"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| {
                    value["host"].as_array().map(|parts| {
                        parts
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(".")
                    })
                })
                .ok_or_else(|| ConversionError::new("POSTMAN_URL", "URL requires raw or host"))?;
            let protocol = value["protocol"].as_str().unwrap_or("https");
            if value.get("protocol").is_none() {
                self.record(
                    pointer,
                    "URL_SCHEME_INFERRED",
                    Classification::Inferred,
                    "Structured URL omits protocol; review proposed HTTPS scheme",
                    Some(value),
                );
            }
            let path = value["path"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| {
                    value["path"].as_array().map(|parts| {
                        parts
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("/")
                    })
                })
                .unwrap_or_default();
            let port = value["port"]
                .as_str()
                .map(|port| format!(":{port}"))
                .unwrap_or_default();
            self.expand(
                &format!("{protocol}://{host}{port}/{}", path.trim_start_matches('/')),
                pointer,
            )
        };
        // Structured query is authoritative: disabled entries must not leak
        // through the raw URL cached by Postman.
        if let Some(query) = value.get("query") {
            let entries = query
                .as_array()
                .ok_or_else(|| ConversionError::new("POSTMAN_URL", "URL query must be an array"))?;
            let fragment = raw.split_once('#').map(|(_, fragment)| fragment.to_owned());
            raw = raw.split(['?', '#']).next().unwrap_or("").into();
            let mut pairs = Vec::new();
            for (index, entry) in entries.iter().enumerate() {
                if entry["disabled"] == true {
                    continue;
                }
                let at = child(&child(pointer, "query"), &index.to_string());
                let name = entry["key"]
                    .as_str()
                    .ok_or_else(|| ConversionError::new("POSTMAN_URL", "Query item needs key"))?;
                pairs.push((
                    self.expand(name, &at),
                    self.expand(entry["value"].as_str().unwrap_or(""), &at),
                ));
            }
            let query = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(pairs)
                .finish();
            if !query.is_empty() {
                raw.push('?');
                raw.push_str(&query);
            }
            if let Some(fragment) = fragment {
                raw.push('#');
                raw.push_str(&fragment);
            }
        }
        Ok(raw)
    }

    fn headers(&mut self, value: Option<&Value>, pointer: &str) -> Result<Vec<Header>> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        if let Some(text) = value.as_str() {
            return text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    let (name, value) = line.split_once(':').ok_or_else(|| {
                        ConversionError::new("POSTMAN_HEADER", "Header line needs a colon")
                    })?;
                    Ok(Header {
                        name: name.trim().into(),
                        value: self.expand(value.trim(), pointer),
                        pointer: pointer.into(),
                    })
                })
                .collect();
        }
        let entries = value.as_array().ok_or_else(|| {
            ConversionError::new("POSTMAN_HEADER", "Headers must be array or string")
        })?;
        let mut result = Vec::new();
        for (index, header) in entries.iter().enumerate() {
            if header["disabled"] == true {
                continue;
            }
            let at = child(pointer, &index.to_string());
            let name = header["key"]
                .as_str()
                .ok_or_else(|| ConversionError::new("POSTMAN_HEADER", "Header needs string key"))?;
            result.push(Header {
                name: self.expand(name, &at),
                value: self.expand(header["value"].as_str().unwrap_or(""), &at),
                pointer: at,
            });
        }
        Ok(result)
    }

    fn body(
        &mut self,
        value: Option<&Value>,
        headers: &[Header],
        pointer: &str,
    ) -> Result<Option<Payload>> {
        let Some(value) = value else {
            return Ok(None);
        };
        if value["disabled"] == true {
            return Ok(None);
        }
        let media = headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case("content-type"))
            .map(|header| header.value.as_str());
        let v3 = value.get("type").is_some() && value.get("mode").is_none();
        if v3 {
            unknown_fields(
                value,
                &["type", "content", "$type"],
                self.sources,
                &self.sources.root,
                pointer,
                self.trace,
            );
        }
        let mode = value[if v3 { "type" } else { "mode" }].as_str();
        let payload = match mode {
            Some("none") if v3 => Ok(None),
            Some(kind)
                if kind == "raw"
                    || v3
                        && matches!(
                            kind,
                            "text" | "json" | "xml" | "html" | "javascript" | "yaml"
                        ) =>
            {
                let key = if v3 { "content" } else { "raw" };
                let raw = self.expand(value[key].as_str().unwrap_or(""), &child(pointer, key));
                let language = value
                    .pointer("/options/raw/language")
                    .and_then(Value::as_str)
                    .or(if v3 { mode } else { None });
                let media = media.or(match language {
                    Some("json") => Some("application/json"),
                    Some("xml") => Some("application/xml"),
                    Some("html") => Some("text/html"),
                    Some("javascript") => Some("application/javascript"),
                    Some("yaml") => Some("application/yaml"),
                    _ => None,
                });
                http::example_payload(&raw, media, &child(pointer, key), self.sources)
            }
            Some("urlencoded" | "formdata") => {
                let multipart = mode == Some("formdata");
                let key = if v3 {
                    "content"
                } else if multipart {
                    "formdata"
                } else {
                    "urlencoded"
                };
                let fields = value[key].as_array().ok_or_else(|| {
                    ConversionError::new("POSTMAN_BODY", "Form body needs fields array")
                })?;
                let mut properties = serde_json::Map::new();
                let mut example = serde_json::Map::new();
                let mut encoding = serde_json::Map::new();
                for (index, field) in fields.iter().enumerate() {
                    if field["disabled"] == true {
                        continue;
                    }
                    let at = child(&child(pointer, key), &index.to_string());
                    let name = field["key"].as_str().ok_or_else(|| {
                        ConversionError::new("POSTMAN_BODY", "Form field needs key")
                    })?;
                    let is_file = field["type"] == "file";
                    let mut schema = if is_file {
                        json!({"type":"string","format":"binary"})
                    } else {
                        json!({"type":"string"})
                    };
                    if let Some(description) = description(field.get("description")) {
                        schema["description"] = json!(description);
                    }
                    if let Some(previous) = properties.get_mut(name) {
                        if previous.get("type") == Some(&json!("array")) {
                        } else {
                            *previous = json!({"type":"array","items":previous.clone()});
                        }
                    } else {
                        properties.insert(name.into(), schema);
                    }
                    if is_file {
                        self.record(&at,"FILE_CONTENT_NOT_READ",Classification::Transformed,"File upload represented by binary schema; local source path is never read",field.get("src"));
                    } else {
                        let observed = self.expand(field["value"].as_str().unwrap_or(""), &at);
                        if let Some(previous) = example.get_mut(name) {
                            if let Some(array) = previous.as_array_mut() {
                                array.push(json!(observed));
                            } else {
                                *previous = json!([previous.clone(), observed]);
                            }
                        } else {
                            example.insert(name.into(), json!(observed));
                        }
                    }
                    if let Some(content_type) = field["contentType"].as_str() {
                        encoding.insert(name.into(), json!({"contentType":content_type}));
                    }
                }
                Ok(Some(Payload {
                    media: media
                        .unwrap_or(if multipart {
                            "multipart/form-data"
                        } else {
                            "application/x-www-form-urlencoded"
                        })
                        .split(';')
                        .next()
                        .unwrap_or("")
                        .into(),
                    schema: json!({"type":"object","properties":properties}),
                    example: if example.is_empty() {
                        None
                    } else {
                        Some(Value::Object(example))
                    },
                    encoding: if encoding.is_empty() {
                        None
                    } else {
                        Some(Value::Object(encoding))
                    },
                    pointer: pointer.into(),
                    inferred: true,
                }))
            }
            Some("file") => {
                self.record(
                    pointer,
                    "FILE_CONTENT_NOT_READ",
                    Classification::Transformed,
                    "File body represented by binary schema; local source path is never read",
                    Some(value),
                );
                Ok(Some(Payload {
                    media: media.unwrap_or("application/octet-stream").into(),
                    schema: json!({"type":"string","format":"binary"}),
                    example: None,
                    encoding: None,
                    pointer: pointer.into(),
                    inferred: false,
                }))
            }
            Some("graphql") => {
                self.record(pointer,"GRAPHQL_REQUEST_OBSERVATION",Classification::Inferred,"GraphQL request export describes an HTTP envelope only. Supply SDL/introspection plus explicit operation mappings to convert its schema.",Some(value));
                let mut payload = value[if v3 { "content" } else { "graphql" }].clone();
                if v3 {
                    for key in ["query", "operationName"] {
                        if let Some(text) = payload[key].as_str() {
                            payload[key] =
                                json!(self.expand(text, &child(&child(pointer, "content"), key)));
                        }
                    }
                }
                if v3 && let Some(variables) = payload["variables"].as_str() {
                    let variables =
                        self.expand(variables, &child(&child(pointer, "content"), "variables"));
                    let parsed = octri_parser::parse_source(
                        "memory://postman/variables.json",
                        variables,
                        &self.config.limits.parser(),
                    )?;
                    if !parsed.document.is_object() {
                        return Err(ConversionError::new(
                            "POSTMAN_BODY",
                            "GraphQL variables must encode a JSON object",
                        ));
                    }
                    payload["variables"] = parsed.document;
                }
                Ok(Some(Payload {
                    media: media.unwrap_or("application/json").into(),
                    schema: crate::schema::infer(&payload),
                    example: Some(payload),
                    encoding: None,
                    pointer: pointer.into(),
                    inferred: true,
                }))
            }
            mode => Err(ConversionError::new(
                "POSTMAN_BODY",
                format!(
                    "Unsupported or missing body mode {mode:?}; source retained, no conversion claimed"
                ),
            )),
        }?;
        v3::authored_body_type(self, value, pointer, payload)
    }

    fn auth(&mut self, value: Option<&Value>, pointer: &str) -> Result<Auth> {
        let Some(value) = value.filter(|value| !value.is_null()) else {
            return Ok(Auth::Unknown);
        };
        let kind = value["type"]
            .as_str()
            .ok_or_else(|| ConversionError::new("POSTMAN_AUTH", "Authentication needs type"))?;
        let settings = variables_from(
            value.get(if value.get("credentials").is_some() {
                "credentials"
            } else {
                kind
            }),
            "key",
        )?;
        let mut required_scopes = Vec::new();
        for key in ["id", "name"] {
            if let Some(metadata) = value.get(key) {
                self.record(&child(pointer,key),"POSTMAN_AUTH_METADATA",Classification::Omitted,
                    "Postman credential-set metadata remains in source; it does not define an OpenAPI security requirement",Some(metadata));
            }
        }
        let mut scheme = match kind {
            "noauth" => {
                if let Some(description) = value.get("description") {
                    self.record(&child(pointer,"description"),"POSTMAN_AUTH_METADATA",Classification::Omitted,
                        "No-auth description remains in source; an empty security requirement has no description field",Some(description));
                }
                return Ok(Auth::None);
            }
            "basic" => json!({"type":"http","scheme":"basic"}),
            "bearer" => json!({"type":"http","scheme":"bearer"}),
            "digest" => json!({"type":"http","scheme":"digest"}),
            "apikey" => {
                let name = settings.get("key").ok_or_else(|| {
                    ConversionError::new("POSTMAN_AUTH", "API key auth needs key name")
                })?;
                let location = settings.get("in").map(String::as_str).unwrap_or("header");
                if !matches!(location, "header" | "query") {
                    return Err(ConversionError::new(
                        "POSTMAN_AUTH",
                        "API key location must be header or query",
                    ));
                }
                json!({"type":"apiKey","in":location,"name":self.expand(name,pointer)})
            }
            "oauth2" => {
                let grant = settings
                    .get("grant_type")
                    .map(String::as_str)
                    .unwrap_or("authorization_code");
                let flow = match grant {
                    "authorization_code" | "authorization_code_with_pkce" => "authorizationCode",
                    "client_credentials" => "clientCredentials",
                    "password_credentials" | "password" => "password",
                    "implicit" => "implicit",
                    _ => "unknown",
                };
                let mut data = json!({"scopes":{}});
                if matches!(flow, "authorizationCode" | "implicit") {
                    if let Some(url) = settings.get("authUrl") {
                        data["authorizationUrl"] = json!(self.expand(url, pointer));
                    } else {
                        self.record(
                            pointer,
                            "OAUTH_URL_MISSING",
                            Classification::Blocked,
                            "OAuth flow requires an explicit authorization URL",
                            Some(value),
                        );
                    }
                }
                if flow != "implicit" {
                    if let Some(url) = settings.get("accessTokenUrl") {
                        data["tokenUrl"] = json!(self.expand(url, pointer));
                    } else {
                        self.record(
                            pointer,
                            "OAUTH_URL_MISSING",
                            Classification::Blocked,
                            "OAuth flow requires an explicit token URL",
                            Some(value),
                        );
                    }
                }
                if let Some(scopes) = settings.get("scope") {
                    let at = auth_setting_pointer(value, pointer, kind, "scope");
                    let scopes = self.expand(scopes, &at);
                    required_scopes = http::oauth_scopes(&scopes);
                    for scope in &required_scopes {
                        data["scopes"][scope] = json!("");
                    }
                }
                if flow == "unknown" {
                    self.record(
                        pointer,
                        "OAUTH_GRANT",
                        Classification::Blocked,
                        "Unknown OAuth grant requires an authored security scheme",
                        Some(value),
                    );
                }
                json!({"type":"oauth2","flows":{flow:data}})
            }
            _ => {
                self.record(pointer,"AUTH_UNREPRESENTABLE",Classification::Blocked,format!("{kind} auth needs an explicit OpenAPI security mapping; no weaker scheme substituted"),Some(value));
                return Ok(Auth::Unknown);
            }
        };
        if let Some(description) = description(value.get("description")) {
            scheme["description"] = json!(description);
        }
        Ok(Auth::scoped_scheme(kind, scheme, required_scopes))
    }

    fn events(&mut self, value: &Value, pointer: &str) {
        if let Some(events) = value.get("event").and_then(Value::as_array) {
            for (index, event) in events.iter().enumerate() {
                if event["disabled"] == true {
                    continue;
                }
                self.record(&child(&child(pointer,"event"),&index.to_string()),"SCRIPT_NOT_EXECUTED",Classification::Omitted,"Postman script retained verbatim in source. Dynamic URL, auth or body mutations cannot be inferred without authored values.",Some(event));
            }
        }
    }

    fn expand(&mut self, raw: &str, pointer: &str) -> String {
        let mut result = raw.to_owned();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..16 {
            if !result.contains("{{") {
                return result;
            }
            if !seen.insert(result.clone()) {
                self.record(
                    pointer,
                    "VARIABLE_CYCLE",
                    Classification::Blocked,
                    "Cyclic variable expansion needs an explicit value",
                    Some(&json!(raw)),
                );
                return result;
            }
            let mut expanded = String::new();
            let mut tail = result.as_str();
            let mut unresolved = false;
            while let Some((before, rest)) = tail.split_once("{{") {
                if expanded.len().saturating_add(before.len())
                    > self.config.limits.max_document_bytes
                {
                    return self.expansion_limit(raw, pointer);
                }
                expanded.push_str(before);
                let Some((name, after)) = rest.split_once("}}") else {
                    unresolved = true;
                    expanded.push_str("{{");
                    expanded.push_str(rest);
                    tail = "";
                    break;
                };
                if let Some(value) = self.variables.get(name) {
                    if expanded.len().saturating_add(value.len())
                        > self.config.limits.max_document_bytes
                    {
                        return self.expansion_limit(raw, pointer);
                    }
                    expanded.push_str(value);
                } else {
                    unresolved = true;
                    expanded.push_str("{{");
                    expanded.push_str(name);
                    expanded.push_str("}}");
                }
                tail = after;
            }
            if expanded.len().saturating_add(tail.len()) > self.config.limits.max_document_bytes {
                return self.expansion_limit(raw, pointer);
            }
            expanded.push_str(tail);
            result = expanded;
            if unresolved {
                self.record(pointer,"VARIABLE_UNRESOLVED",Classification::Blocked,"Unresolved or dynamic Postman variable. Supply explicit variables; runtime scripts are never executed.",Some(&json!(raw)));
                return result;
            }
        }
        self.record(
            pointer,
            "VARIABLE_DEPTH",
            Classification::Blocked,
            "Variable expansion exceeds 16 passes",
            Some(&json!(raw)),
        );
        result
    }

    fn expansion_limit(&mut self, raw: &str, pointer: &str) -> String {
        self.record(
            pointer,
            "VARIABLE_SIZE",
            Classification::Blocked,
            "Expanded value exceeds configured source byte limit",
            Some(&json!(raw)),
        );
        raw.into()
    }

    fn record(
        &mut self,
        pointer: &str,
        code: &str,
        class: Classification,
        message: impl Into<String>,
        evidence: Option<&Value>,
    ) {
        self.trace.record(
            self.sources,
            &self.sources.root,
            pointer,
            &[],
            code,
            class,
            message,
            evidence,
        );
    }
}

fn auth_setting_pointer(value: &Value, pointer: &str, kind: &str, key: &str) -> String {
    let kind = if value.get("credentials").is_some() {
        "credentials"
    } else {
        kind
    };
    value
        .get(kind)
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries.iter().position(|entry| {
                entry["key"] == key && entry["disabled"] != true && entry["enabled"] != false
            })
        })
        .map_or_else(
            || pointer.to_owned(),
            |index| child(&child(&child(pointer, kind), &index.to_string()), "value"),
        )
}

fn description(value: Option<&Value>) -> Option<String> {
    value
        .and_then(|value| value.as_str().or_else(|| value["content"].as_str()))
        .map(str::to_owned)
}
