//! AsyncAPI 2.x/3.x contracts require explicit HTTP endpoint and payload bindings.
use super::{
    mapped,
    references::{self, Located, Schemas},
};
use crate::{Classification, Config, ConversionError, Result};
use crate::{
    source::{Sources, child},
    trace::Trace,
};
mod payload;
pub(super) use payload::schema_dialect;
use payload::{Payload, combine, messages};

use octri_parser::Address;
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(crate) fn convert(sources: &mut Sources, config: &Config, trace: &mut Trace) -> Result<Value> {
    let uri = sources.root.clone();
    let document = sources.parse(&uri, false)?;
    let version = document["asyncapi"]
        .as_str()
        .ok_or_else(|| ConversionError::new("ASYNCAPI_VERSION", "Source needs AsyncAPI version"))?;
    if !version.starts_with("2.") && !version.starts_with("3.") {
        return Err(ConversionError::new(
            "ASYNCAPI_VERSION",
            "Expected a published AsyncAPI 2.x or 3.x document",
        ));
    }
    let mut schemas = Schemas::default();
    let mut operations = Vec::new();
    if version.starts_with("2.") {
        for (name, channel) in document
            .get("channels")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let at = child("/channels", name);
            let channel = references::resolve(
                sources,
                Located {
                    value: channel.clone(),
                    address: Address {
                        uri: uri.clone(),
                        pointer: at,
                    },
                },
                trace,
            )?;
            for action in ["publish", "subscribe"] {
                let Some(operation) = channel.value.get(action) else {
                    continue;
                };
                let pointer = child(&channel.address.pointer, action);
                let operation = references::resolve(
                    sources,
                    Located {
                        value: operation.clone(),
                        address: Address {
                            uri: channel.address.uri.clone(),
                            pointer,
                        },
                    },
                    trace,
                )?;
                let key = format!("{action} {name}");
                let payload = messages(
                    operation.value.get("message"),
                    &Address {
                        uri: operation.address.uri.clone(),
                        pointer: child(&operation.address.pointer, "message"),
                    },
                    sources,
                    trace,
                    &mut schemas,
                    replaces_payload(&key, config),
                )?;
                operations.push(bind(&key, operation, payload, config, sources, trace)?);
            }
        }
    } else {
        for (name, operation) in document
            .get("operations")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let at = child("/operations", name);
            let operation = references::resolve(
                sources,
                Located {
                    value: operation.clone(),
                    address: Address {
                        uri: uri.clone(),
                        pointer: at,
                    },
                },
                trace,
            )?;
            if !matches!(operation.value["action"].as_str(), Some("send" | "receive")) {
                return Err(ConversionError::new(
                    "ASYNCAPI_ACTION",
                    "Operation action must be send or receive",
                ));
            }
            let channel = references::resolve(
                sources,
                Located {
                    value: operation.value["channel"].clone(),
                    address: Address {
                        uri: operation.address.uri.clone(),
                        pointer: child(&operation.address.pointer, "channel"),
                    },
                },
                trace,
            )?;
            let mut payloads = Vec::new();
            if let Some(messages_list) = operation.value.get("messages").and_then(Value::as_array) {
                for (index, message) in messages_list.iter().enumerate() {
                    payloads.push(messages(
                        Some(message),
                        &Address {
                            uri: operation.address.uri.clone(),
                            pointer: child(
                                &child(&operation.address.pointer, "messages"),
                                &index.to_string(),
                            ),
                        },
                        sources,
                        trace,
                        &mut schemas,
                        replaces_payload(name, config),
                    )?);
                }
            } else {
                for (message_name, message) in channel
                    .value
                    .get("messages")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    payloads.push(messages(
                        Some(message),
                        &Address {
                            uri: channel.address.uri.clone(),
                            pointer: child(
                                &child(&channel.address.pointer, "messages"),
                                message_name,
                            ),
                        },
                        sources,
                        trace,
                        &mut schemas,
                        replaces_payload(name, config),
                    )?);
                }
            }
            let payload = combine(payloads);
            operations.push(bind(name, operation, payload, config, sources, trace)?);
        }
    }
    if operations.is_empty() {
        return Err(ConversionError::new(
            "ASYNCAPI_OPERATIONS",
            "No declared AsyncAPI operations found",
        ));
    }
    let definitions = schemas.finish(sources, Default::default())?;
    check_unused_component_schemas(sources, trace);
    mapped::emit(
        document
            .pointer("/info/title")
            .and_then(Value::as_str)
            .unwrap_or("Converted AsyncAPI contract"),
        document.pointer("/info/version").and_then(Value::as_str),
        Value::Object(definitions),
        operations,
        sources,
        config,
        trace,
    )
}

fn check_unused_component_schemas(sources: &Sources, trace: &mut Trace) {
    let imported: BTreeSet<Address> = sources
        .origins
        .values()
        .filter_map(|origin| {
            let name = origin
                .pointer
                .strip_prefix("/components/schemas/")?
                .split('/')
                .next()?;
            Some(Address {
                uri: origin.uri.clone(),
                pointer: format!("/components/schemas/{name}"),
            })
        })
        .collect();
    for source in sources.documents.values() {
        if source.document.get("asyncapi").is_none() {
            continue;
        }
        for (name, schema) in source
            .document
            .pointer("/components/schemas")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let pointer = child("/components/schemas", name);
            if !imported.contains(&Address {
                uri: source.uri.clone(),
                pointer: pointer.clone(),
            }) {
                trace.record(
                        sources,
                        &source.uri,
                        &pointer,
                        &[],
                        "ASYNCAPI_UNUSED_SCHEMA",
                        Classification::Omitted,
                        "No selected HTTP payload uses this component schema. It remains in the original source; review its omission from the HTTP contract.",
                        Some(schema),
                );
            }
        }
    }
}

/// A replacement covers only the selected payload direction. A response
/// schema must never suppress diagnostics for a source-bound request payload.
fn replaces_payload(key: &str, config: &Config) -> bool {
    config
        .http_mappings
        .get(key)
        .is_some_and(|mapping| match mapping.payload_location.as_deref() {
            Some("request") => mapping.request_schema.is_some(),
            Some("response") => mapping.response_schema.is_some(),
            None => mapping.request_schema.is_some() || mapping.response_schema.is_some(),
            _ => false,
        })
}

fn bind(
    key: &str,
    operation: Located,
    payload: Payload,
    config: &Config,
    sources: &Sources,
    trace: &mut Trace,
) -> Result<mapped::Operation> {
    let payload_location_required = payload.declared;
    let payload = payload.schema;
    let mut input = None;
    let mut output = None;
    if let Some(mapping) = config.http_mappings.get(key) {
        match mapping.payload_location.as_deref() {
            Some("request") => input = payload,
            Some("response") => output = payload,
            Some(_) => {
                return Err(ConversionError::new(
                    "ASYNCAPI_PAYLOAD_BINDING",
                    "payloadLocation must be request or response",
                ));
            }
            None if payload_location_required
                && mapping.request_schema.is_none()
                && mapping.response_schema.is_none() =>
            {
                trace.record(
                    sources,
                    &operation.address.uri,
                    &operation.address.pointer,
                    &[],
                    "ASYNCAPI_PAYLOAD_BINDING",
                    Classification::Blocked,
                    "Set payloadLocation to request or response. Publish/subscribe direction alone does not establish an HTTP endpoint contract.",
                    Some(&json!(key)),
                );
            }
            None => {}
        }
    }
    for field in ["bindings", "security", "traits", "reply"] {
        if let Some(value) = operation.value.get(field) {
            trace.record(sources,&operation.address.uri,&child(&operation.address.pointer,field),&[],"ASYNCAPI_PROTOCOL_BEHAVIOR",Classification::Omitted,format!("{field} protocol behavior retained in source. Explicit HTTP mappings define emitted transport; review equivalence, authentication and reply lifecycle."),Some(value));
        }
    }
    Ok(mapped::Operation {
        uri: operation.address.uri,
        key: key.into(),
        pointer: operation.address.pointer,
        description: operation.value["description"].as_str().map(str::to_owned),
        input,
        output,
        request_streaming: false,
        response_streaming: false,
        payload_location_required,
    })
}
