//! Observed HTTP requests share emission, never pretend observations are contracts.

use super::{common::operation_id, http_method::HttpMethod};
use crate::{Classification, Config, ConversionError, Result};
use crate::{
    schema,
    source::{Sources, child, hash},
    trace::Trace,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub(crate) struct Header {
    pub name: String,
    pub value: String,
    pub pointer: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Payload {
    pub media: String,
    pub schema: Value,
    pub example: Option<Value>,
    pub encoding: Option<Value>,
    pub pointer: String,
    pub inferred: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Response {
    pub status: String,
    pub description: String,
    pub headers: Vec<Header>,
    pub payload: Option<Payload>,
    pub pointer: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) enum Auth {
    #[default]
    Unknown,
    None,
    Schemes(BTreeMap<String, AuthScheme>),
}

/// A scheme definition and the scopes required by this particular request.
/// Available OAuth scopes and operation requirements have different meanings.
#[derive(Debug, Clone)]
pub(crate) struct AuthScheme {
    pub definition: Value,
    pub scopes: Vec<String>,
}

impl Auth {
    pub fn scheme(name: &str, definition: Value) -> Self {
        Self::scoped_scheme(name, definition, Vec::new())
    }

    pub fn scoped_scheme(name: &str, definition: Value, scopes: Vec<String>) -> Self {
        Self::Schemes(BTreeMap::from([(
            name.into(),
            AuthScheme { definition, scopes },
        )]))
    }
}

/// Scope requirements are a set; retain first occurrence order for stable output.
pub(crate) fn oauth_scopes(value: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    value
        .split_whitespace()
        .filter(|scope| seen.insert(*scope))
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Clone)]
pub(crate) struct Observation {
    pub pointer: String,
    pub name: String,
    pub description: Option<String>,
    pub method: String,
    pub url: String,
    pub headers: Vec<Header>,
    pub path_variables: BTreeMap<String, String>,
    pub body: Option<Payload>,
    pub responses: Vec<Response>,
    pub tags: Vec<String>,
    pub auth: Auth,
}

struct RequestUrl {
    server: String,
    path: String,
    query: Vec<(String, String)>,
}

pub(crate) struct Builder<'a> {
    pub sources: &'a Sources,
    pub config: &'a Config,
    pub trace: &'a mut Trace,
    document: Value,
    operations: BTreeMap<String, String>,
    candidates: BTreeMap<String, Vec<String>>,
    ids: BTreeSet<String>,
}

impl<'a> Builder<'a> {
    pub fn new(
        title: &str,
        sources: &'a Sources,
        config: &'a Config,
        trace: &'a mut Trace,
    ) -> Self {
        if config.api_version.is_none() {
            trace.record(sources, &sources.root, "", &["/info/version".into()],
                "API_VERSION_INFERRED", Classification::Inferred,
                "Observed requests do not declare an API release version. Set apiVersion or review the proposed info.version before publishing.", None);
        }
        Self {
            sources,
            config,
            trace,
            document: json!({"openapi":"3.1.2","info":{"title":config.title.as_deref().unwrap_or(title),"version":config.api_version.as_deref().unwrap_or("1.0.0")},"paths":{},"components":{"securitySchemes":{}}}),
            operations: BTreeMap::new(),
            candidates: BTreeMap::new(),
            ids: BTreeSet::new(),
        }
    }

    pub fn add(&mut self, request: Observation) -> Result<()> {
        let method = HttpMethod::parse(&request.method).ok_or_else(|| {
            ConversionError::new(
                "HTTP_METHOD",
                format!(
                    "Invalid HTTP method token {:?} at {}",
                    request.method, request.pointer
                ),
            )
        })?;
        let RequestUrl {
            server,
            path,
            query,
        } = self.url(&request)?;
        let identity = format!("{} {path}", request.method);
        self.candidates
            .entry(identity.clone())
            .or_default()
            .push(request.pointer.clone());
        if let Some(selected) = self.config.duplicate_choices.get(&identity)
            && selected != &request.pointer
        {
            self.record(&request.pointer, &[], "DUPLICATE_NOT_SELECTED", Classification::Omitted,
                    format!("Caller selected {selected} for {identity}; this request remains in original sources"), None);
            return Ok(());
        }
        if let Some(previous) = self.operations.get(&identity).cloned() {
            self.record(&request.pointer, &[], "DUPLICATE_OPERATION", Classification::Blocked,
                format!("Multiple requests define {identity}: {previous}, {}. Select a source explicitly.", request.pointer), None);
            return Ok(());
        }
        self.operations.insert(identity, request.pointer.clone());
        let at = method.pointer(&path);
        let mut id = operation_id(&request.method, &path);
        if !self.ids.insert(id.clone()) {
            id.push_str(&hash(request.pointer.as_bytes())[..12]);
            self.ids.insert(id.clone());
        }
        let mut operation = json!({"operationId":id,"summary":request.name,"servers":[{"url":server}],"responses":{}});
        if let Some(description) = &request.description {
            operation["description"] = json!(description);
        }
        if !request.tags.is_empty() {
            operation["tags"] = json!(request.tags);
        }
        let mut parameters = Vec::new();
        let mut names = BTreeSet::new();
        for segment in path.split('{').skip(1) {
            if let Some((name, _)) = segment.split_once('}') {
                if name.is_empty() || !names.insert(("path".to_owned(), name.to_owned())) {
                    continue;
                }
                let mut parameter =
                    json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}});
                if let Some(example) = request.path_variables.get(name) {
                    parameter["example"] = json!(example);
                }
                parameters.push(parameter);
            }
        }
        let mut query_values: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, value) in query {
            query_values.entry(name).or_default().push(value);
        }
        for (name, values) in query_values {
            let parameter = if values.len() == 1 {
                json!({"name":name,"in":"query","schema":{"type":"string"},"example":values[0]})
            } else {
                json!({"name":name,"in":"query","style":"form","explode":true,"schema":{"type":"array","items":{"type":"string"}},"example":values})
            };
            names.insert(("query".into(), name));
            parameters.push(parameter);
        }
        for header in &request.headers {
            let name = header.name.to_ascii_lowercase();
            if matches!(
                name.as_str(),
                "accept"
                    | "content-type"
                    | "authorization"
                    | "host"
                    | "content-length"
                    | "connection"
                    | "transfer-encoding"
                    | "cookie"
            ) {
                self.record(&header.pointer,std::slice::from_ref(&at),"TRANSPORT_HEADER",Classification::Omitted,format!("Observed {} header is retained in original source; it is not an OpenAPI header parameter",header.name),Some(&json!(header.value)));
                continue;
            }
            if names.insert(("header".into(), name)) {
                parameters.push(json!({"name":header.name,"in":"header","schema":{"type":"string"},"example":header.value}));
            } else {
                self.record(
                    &header.pointer,
                    std::slice::from_ref(&at),
                    "REPEATED_HEADER",
                    Classification::Blocked,
                    "Repeated HTTP header needs an authored serialization mapping",
                    Some(&json!(header.value)),
                );
            }
        }
        if !parameters.is_empty() {
            operation["parameters"] = Value::Array(parameters);
        }
        if let Some(payload) = &request.body {
            let payload_at = child(
                &child(&child(&at, "requestBody"), "content"),
                &payload.media,
            );
            operation["requestBody"] =
                json!({"content":{&payload.media:self.payload(payload,&payload_at)}});
        }
        match &request.auth {
            Auth::None=>operation["security"]=json!([]),
            Auth::Unknown=>self.record(&request.pointer,std::slice::from_ref(&at),"AUTH_UNDECLARED",Classification::Inferred,"Authentication contract is not established by this request. Review security before publishing.",None),
            Auth::Schemes(schemes)=>{
                let mut requirement=Map::new();
                for (name, auth) in schemes {
                    let unique = format!("{}_{}", name, &hash(auth.definition.to_string().as_bytes())[..12]);
                    self.document["components"]["securitySchemes"][&unique] = auth.definition.clone();
                    requirement.insert(unique, json!(auth.scopes));
                }
                operation["security"] = json!([requirement]);
            }
        }
        let mut responses = Map::new();
        for response in &request.responses {
            if !valid_status(&response.status) {
                return Err(ConversionError::new(
                    "RESPONSE_STATUS",
                    format!("Invalid response status {}", response.status),
                ));
            }
            let response_at = child(&child(&at, "responses"), &response.status);
            let mut item = json!({"description":response.description});
            if let Some(payload) = &response.payload {
                item["content"] = json!({&payload.media:self.payload(payload,&child(&child(&response_at,"content"),&payload.media))});
            }
            let mut headers = Map::new();
            for header in &response.headers {
                if matches!(
                    header.name.to_ascii_lowercase().as_str(),
                    "content-type" | "content-length" | "transfer-encoding" | "connection"
                ) {
                    continue;
                }
                headers.insert(
                    header.name.clone(),
                    json!({"schema":{"type":"string"},"example":header.value}),
                );
            }
            if !headers.is_empty() {
                item["headers"] = Value::Object(headers);
            }
            if let Some(previous) = responses.get(&response.status) {
                if !octri_parser::numbers::equivalent(previous, &item) {
                    let merged = super::response_variants::merge(previous, &item);
                    responses.insert(response.status.clone(), merged);
                    self.record(&response.pointer, &[response_at], "RESPONSE_VARIANTS", Classification::Inferred,
                        "Observed responses share a status. Their media types, schemas, examples and headers were combined conservatively. Review the union or supply an authored response override; observations do not establish required response coverage.", Some(&item));
                }
            } else {
                responses.insert(response.status.clone(), item);
            }
        }
        if responses.is_empty() {
            responses.insert(
                "default".into(),
                json!({"description":"Response contract not supplied"}),
            );
            self.record(&request.pointer,&[child(&at,"responses")],"RESPONSE_UNDECLARED",Classification::Inferred,"No response contract was supplied. Default response is a placeholder requiring review, not evidence of a success status.",None);
        }
        operation["responses"] = Value::Object(responses);
        method.insert(&mut self.document, &path, operation);
        self.record(&request.pointer,&[at],"OBSERVED_HTTP_REQUEST",Classification::Inferred,"Observed requests establish examples and proposed types; required fields, constraints and response coverage need contract review",None);
        Ok(())
    }

    fn payload(&mut self, payload: &Payload, at: &str) -> Value {
        let mut item = json!({"schema":payload.schema});
        if let Some(example) = &payload.example {
            item["example"] = example.clone();
        }
        if let Some(encoding) = &payload.encoding {
            item["encoding"] = encoding.clone();
        }
        if payload.inferred {
            self.record(&payload.pointer,&[child(at,"schema")],"SCHEMA_FROM_EXAMPLE",Classification::Inferred,"Schema inferred from observed example types; no enum, required fields or validation constraints were invented",payload.example.as_ref());
        }
        item
    }

    fn url(&mut self, request: &Observation) -> Result<RequestUrl> {
        let raw = request.url.trim();
        if raw.contains("{{") || raw.contains("}}") {
            self.record(
                &request.pointer,
                &[],
                "UNRESOLVED_VARIABLE",
                Classification::Blocked,
                "URL contains unresolved source variables. Supply explicit variable values.",
                Some(&json!(raw)),
            );
        }
        let parsed = if raw.starts_with('/') {
            let base = self.config.base_url.as_deref().ok_or_else(|| {
                ConversionError::new(
                    "BASE_URL_REQUIRED",
                    "Relative request URL needs an explicit baseUrl",
                )
            })?;
            url::Url::parse(base).and_then(|base| base.join(raw))
        } else {
            url::Url::parse(raw)
        }
        .map_err(|error| ConversionError::new("REQUEST_URL", error.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(ConversionError::new(
                "REQUEST_URL",
                "Request URLs require HTTP(S), a host and no embedded credentials",
            ));
        }
        if parsed.fragment().is_some() {
            self.record(
                &request.pointer,
                &[],
                "URL_FRAGMENT",
                Classification::Omitted,
                "URI fragment is not sent in HTTP requests",
                Some(&json!(raw)),
            );
        }
        let mut authority = parsed.clone();
        authority.set_path("");
        authority.set_query(None);
        authority.set_fragment(None);
        let mut request_path = parsed.path();
        let server = if let Some(base) = &self.config.base_url {
            let base = url::Url::parse(base)
                .map_err(|error| ConversionError::new("BASE_URL", error.to_string()))?;
            if !matches!(base.scheme(), "http" | "https")
                || base.host_str().is_none()
                || !base.username().is_empty()
                || base.password().is_some()
                || base.query().is_some()
                || base.fragment().is_some()
            {
                return Err(ConversionError::new(
                    "BASE_URL",
                    "baseUrl requires HTTP(S), a host, and no credentials, query or fragment",
                ));
            }
            let prefix = base.path().trim_end_matches('/');
            if !prefix.is_empty()
                && (request_path == prefix
                    || request_path
                        .strip_prefix(prefix)
                        .is_some_and(|suffix| suffix.starts_with('/')))
            {
                request_path = &request_path[prefix.len()..];
            }
            self.record(&request.pointer, &["/paths".into()], "SERVER_OVERRIDE", Classification::Transformed,
                "Caller supplied the HTTP server base. A matching path prefix was separated from the operation route so it is not duplicated.", Some(&json!(raw)));
            base.as_str().trim_end_matches('/').to_owned()
        } else {
            authority.as_str().trim_end_matches('/').to_owned()
        };
        let path = request_path
            .replace("%7B", "{")
            .replace("%7D", "}")
            .replace("%7b", "{")
            .replace("%7d", "}");
        let path = path
            .split('/')
            .map(|part| {
                part.strip_prefix(':')
                    .map_or_else(|| part.to_owned(), |name| format!("{{{name}}}"))
            })
            .collect::<Vec<_>>()
            .join("/");
        if path.matches('{').count() != path.matches('}').count() {
            return Err(ConversionError::new(
                "REQUEST_PATH",
                "Unbalanced path parameter braces",
            ));
        }
        Ok(RequestUrl {
            server,
            path: if path.is_empty() { "/".into() } else { path },
            query: parsed
                .query_pairs()
                .map(|(name, value)| (name.into_owned(), value.into_owned()))
                .collect(),
        })
    }

    pub fn record(
        &mut self,
        pointer: &str,
        output: &[String],
        code: &str,
        class: Classification,
        message: impl Into<String>,
        evidence: Option<&Value>,
    ) {
        self.trace.record(
            self.sources,
            &self.sources.root,
            pointer,
            output,
            code,
            class,
            message,
            evidence,
        );
    }

    pub fn finish(self) -> Value {
        for (identity, pointers) in self.candidates {
            let selected = self.config.duplicate_choices.get(&identity).cloned();
            if selected
                .as_ref()
                .is_some_and(|choice| !pointers.contains(choice))
            {
                self.trace.record(self.sources, &self.sources.root, "", &[],
                    "DUPLICATE_SELECTION_MISSING", Classification::Blocked,
                    format!("Selected source for {identity} is absent. Choose a current source candidate."),
                    selected.as_ref().map(|value| Value::String(value.clone())).as_ref());
            }
            if pointers.len() > 1 || selected.is_some() {
                self.trace.describe_duplicate(crate::DuplicateRequest {
                    identity,
                    candidates: pointers
                        .into_iter()
                        .map(|pointer| crate::DuplicateCandidate {
                            source: self.sources.location(&self.sources.root, &pointer),
                            pointer,
                        })
                        .collect(),
                    selected,
                });
            }
        }
        self.document
    }
}

pub(crate) fn valid_status(status: &str) -> bool {
    status == "default"
        || (status.len() == 3
            && status.as_bytes()[0].is_ascii_digit()
            && ('1'..='5').contains(&status.chars().next().unwrap_or('0'))
            && status.as_bytes()[1..].iter().all(u8::is_ascii_digit))
}

pub(crate) fn example_payload(
    text: &str,
    media: Option<&str>,
    pointer: &str,
    sources: &Sources,
) -> Result<Option<Payload>> {
    if text.is_empty() {
        return Ok(None);
    }
    let media = media
        .unwrap_or(if text.trim_start().starts_with(['{', '[']) {
            "application/json"
        } else {
            "text/plain"
        })
        .split(';')
        .next()
        .unwrap_or("text/plain")
        .trim()
        .to_owned();
    if media.is_empty() {
        return Err(ConversionError::new(
            "MEDIA_TYPE",
            "Empty payload media type",
        ));
    }
    let example = if media == "application/json" || media.ends_with("+json") {
        octri_parser::parse_source(&sources.root, text.into(), &sources.limits.parser())?.document
    } else {
        json!(text)
    };
    Ok(Some(Payload {
        media,
        schema: schema::infer(&example),
        example: Some(example),
        encoding: None,
        pointer: pointer.into(),
        inferred: true,
    }))
}
