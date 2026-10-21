//! Protocol-independent HTTP bindings. Endpoints only come from caller mappings.
use super::{common::operation_id, http_method::HttpMethod};
use crate::{Classification, Config, ConversionError, Result};
use crate::{
    source::{Sources, child},
    trace::Trace,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

mod streaming;

#[derive(Debug)]
pub(crate) struct Operation {
    pub key: String,
    pub pointer: String,
    pub uri: String,
    pub description: Option<String>,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub request_streaming: bool,
    pub response_streaming: bool,
    pub payload_location_required: bool,
}

pub(super) fn describe(operations: &[Operation], sources: &Sources, trace: &mut Trace) {
    for operation in operations {
        let input = operation.input.as_ref();
        trace.describe_mapping(crate::MappingRequest {
            key: operation.key.clone(),
            source: sources.location(&operation.uri, &operation.pointer),
            input_fields: input
                .and_then(|schema| schema.get("properties"))
                .and_then(Value::as_object)
                .map(|properties| properties.keys().cloned().collect())
                .unwrap_or_default(),
            has_input: input.is_some(),
            has_output: operation.output.is_some(),
            request_stream_required: operation.request_streaming,
            response_stream_required: operation.response_streaming,
            payload_location_required: operation.payload_location_required,
        });
    }
}

pub(crate) fn emit(
    title: &str,
    source_version: Option<&str>,
    schemas: Value,
    operations: Vec<Operation>,
    sources: &Sources,
    config: &Config,
    trace: &mut Trace,
) -> Result<Value> {
    let version = config.api_version.as_deref().or(source_version);
    let mut document = json!({"openapi":"3.1.2","info":{"title":config.title.as_deref().unwrap_or(title),"version":version.unwrap_or("1.0.0")},"paths":{},"components":{"schemas":schemas,"securitySchemes":config.security_schemes}});
    if version.is_none() {
        trace.record(sources,&sources.root,"",&["/info/version".into()],"API_VERSION_INFERRED",Classification::Inferred,"Source does not declare an API release version. Set apiVersion or review the proposed info.version before publishing.",None);
    }
    let mut ids = BTreeSet::new();
    let mut used = BTreeSet::new();
    describe(&operations, sources, trace);
    for operation in operations {
        let Some(mapping) = config.http_mappings.get(&operation.key) else {
            trace.record(sources,&operation.uri,&operation.pointer,&[],"HTTP_MAPPING_REQUIRED",Classification::Blocked,format!("Provide explicit HTTP method, path, server, response status and parameter bindings for {}",operation.key),Some(&json!(operation.key)));
            continue;
        };
        if (operation.request_streaming && mapping.request_stream.is_none())
            || (operation.response_streaming && mapping.response_stream.is_none())
        {
            trace.record(sources,&operation.uri,&operation.pointer,&[],"HTTP_STREAM_MAPPING_REQUIRED",Classification::Blocked,"Streaming source operation requires explicit HTTP framing and lifecycle for every streaming direction.",Some(&json!(operation.key)));
        }
        // A streaming request is a sequence of whole source messages. Splitting
        // its fields into unrelated path/query parameters changes that contract.
        if mapping.request_stream.is_some() && !mapping.parameter_bindings.is_empty() {
            return Err(ConversionError::new(
                "HTTP_STREAM_PARAMETERS",
                "Streaming request messages cannot also be split into parameter bindings",
            ));
        }
        used.insert(operation.key.clone());
        let method = HttpMethod::parse(&mapping.method).ok_or_else(|| {
            ConversionError::new(
                "HTTP_MAPPING",
                format!("Invalid HTTP method token for {}", operation.key),
            )
        })?;
        if !mapping.path.starts_with('/')
            || mapping.path.contains(['?', '#'])
            || !super::http::valid_status(&mapping.response_status)
        {
            return Err(ConversionError::new(
                "HTTP_MAPPING",
                format!("Invalid HTTP mapping for {}", operation.key),
            ));
        }
        let server = url::Url::parse(&mapping.server)
            .map_err(|error| ConversionError::new("HTTP_MAPPING", error.to_string()))?;
        if !matches!(server.scheme(), "http" | "https")
            || server.host_str().is_none()
            || !server.username().is_empty()
            || server.password().is_some()
            || server.query().is_some()
            || server.fragment().is_some()
        {
            return Err(ConversionError::new(
                "HTTP_MAPPING",
                "Mapping server needs an absolute HTTP(S) URL without credentials, query or fragment",
            ));
        }
        let at = method.pointer(&mapping.path);
        if document.pointer(&at).is_some() {
            return Err(ConversionError::new(
                "HTTP_MAPPING_COLLISION",
                format!("Mappings share {} {}", mapping.method, mapping.path),
            ));
        }
        let mut id = operation_id(&mapping.method, &mapping.path);
        if !ids.insert(id.clone()) {
            id.push_str(&crate::source::hash(operation.key.as_bytes())[..12]);
            ids.insert(id.clone());
        }
        let mut emitted = json!({"operationId":id,"summary":operation.key,"servers":[{"url":mapping.server}],"responses":{&mapping.response_status:{"description":"Mapped protocol response"}}});
        if let Some(description) = operation.description {
            emitted["description"] = json!(description);
        }
        let mut input = mapping.request_schema.clone().or(operation.input);
        let mut parameters = Vec::new();
        let mut path_names = BTreeSet::new();
        for (field, binding) in &mapping.parameter_bindings {
            if !matches!(
                binding.location.as_str(),
                "path" | "query" | "header" | "cookie"
            ) || binding.name.is_empty()
            {
                return Err(ConversionError::new(
                    "PARAMETER_BINDING",
                    "Bindings require a name and path/query/header/cookie location",
                ));
            }
            let schema=input.as_ref().and_then(|input|input.get("properties")).and_then(|properties|properties.get(field)).cloned().ok_or_else(||ConversionError::new("PARAMETER_BINDING",format!("Input property {field} is not present; supply an explicit requestSchema when binding referenced protocol types")))?;
            let required = binding.location == "path"
                || input
                    .as_ref()
                    .and_then(|input| input.get("required"))
                    .and_then(Value::as_array)
                    .is_some_and(|required| required.contains(&json!(field)));
            parameters.push(json!({"name":binding.name,"in":binding.location,"required":required,"schema":schema}));
            if binding.location == "path" {
                path_names.insert(binding.name.clone());
            }
            if let Some(input) = input.as_mut() {
                if let Some(properties) = input.get_mut("properties").and_then(Value::as_object_mut)
                {
                    properties.remove(field);
                }
                if let Some(required) = input.get_mut("required").and_then(Value::as_array_mut) {
                    required.retain(|name| name != field);
                }
            }
        }
        let mut expected = BTreeSet::new();
        for part in mapping.path.split('{').skip(1) {
            let Some((name, _)) = part.split_once('}') else {
                return Err(ConversionError::new(
                    "PARAMETER_BINDING",
                    "Unclosed path template",
                ));
            };
            expected.insert(name.to_owned());
        }
        if expected != path_names {
            return Err(ConversionError::new(
                "PARAMETER_BINDING",
                format!(
                    "Every path template needs an explicit input-field binding: expected {expected:?}, supplied {path_names:?}"
                ),
            ));
        }
        if !parameters.is_empty() {
            emitted["parameters"] = json!(parameters);
        }
        if let Some(schema) = input.filter(|schema| {
            mapping.request_stream.is_some()
                || !schema
                    .get("properties")
                    .and_then(Value::as_object)
                    .is_some_and(|properties| properties.is_empty())
        }) {
            if matches!(mapping.method.as_str(), "GET" | "HEAD") {
                trace.record(sources,&operation.uri,&operation.pointer,std::slice::from_ref(&at),"HTTP_INPUT_BINDINGS_REQUIRED",Classification::Blocked,"GET/HEAD mapping has unmapped protocol input. Bind each input to path/query/header/cookie or choose a body-carrying method.",Some(&schema));
            } else {
                emitted["requestBody"] = json!({"required":true,"content":streaming::content(schema,mapping.request_stream.as_ref(),config,&operation.uri,&operation.pointer,&child(&at,"requestBody"),sources,trace)?});
            }
        }
        if let Some(schema) = mapping.response_schema.clone().or(operation.output) {
            emitted["responses"][&mapping.response_status]["content"] = streaming::content(
                schema,
                mapping.response_stream.as_ref(),
                config,
                &operation.uri,
                &operation.pointer,
                &child(&child(&at, "responses"), &mapping.response_status),
                sources,
                trace,
            )?;
        }
        if let Some(security) = &mapping.security {
            emitted["security"] = security.clone();
        } else {
            trace.record(sources,&operation.uri,&operation.pointer,&[child(&at,"security")],"MAPPED_AUTH_UNDECLARED",Classification::Inferred,"HTTP authentication is not established by a non-HTTP protocol. Supply security requirements or review absent authentication.",None);
        }
        method.insert(&mut document, &mapping.path, emitted);
        trace.record(sources,&operation.uri,&operation.pointer,&[at],"EXPLICIT_HTTP_MAPPING",Classification::Transformed,"Caller-authored HTTP binding applied to declared source operation; source protocol contract remains in original artifact",Some(&json!(mapping)));
    }
    if let Some(key) = config.http_mappings.keys().find(|key| !used.contains(*key)) {
        return Err(ConversionError::new(
            "UNKNOWN_HTTP_MAPPING",
            format!("Mapping {key} does not identify a source operation"),
        ));
    }
    Ok(document)
}
