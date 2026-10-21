//! AsyncAPI message wrappers, payload dialect checks and physical schema imports.
use super::super::references::{self, Located, Schemas};
use crate::{Classification, ConversionError, Result};
use crate::{
    source::{Sources, child},
    trace::Trace,
};
use octri_parser::Address;
use serde_json::{Value, json};

/// Payload presence remains distinct from its projected schema. An unsupported
/// payload still needs an explicit HTTP direction in the mapping inventory.
#[derive(Debug, Default)]
pub(super) struct Payload {
    pub schema: Option<Value>,
    pub declared: bool,
}

pub(super) fn messages(
    value: Option<&Value>,
    address: &Address,
    sources: &mut Sources,
    trace: &mut Trace,
    schemas: &mut Schemas,
    replace_payload: bool,
) -> Result<Payload> {
    let Some(value) = value else {
        return Ok(Payload::default());
    };
    let message = references::resolve(
        sources,
        Located {
            value: value.clone(),
            address: address.clone(),
        },
        trace,
    )?;
    if let Some(variants) = message.value.get("oneOf").and_then(Value::as_array) {
        let mut payloads = Vec::new();
        for (index, variant) in variants.iter().enumerate() {
            payloads.push(messages(
                Some(variant),
                &Address {
                    uri: message.address.uri.clone(),
                    pointer: child(
                        &child(&message.address.pointer, "oneOf"),
                        &index.to_string(),
                    ),
                },
                sources,
                trace,
                schemas,
                replace_payload,
            )?);
        }
        return Ok(combine(payloads));
    }
    for field in ["headers", "correlationId", "bindings", "traits", "examples"] {
        if let Some(value) = message.value.get(field) {
            trace.record(sources,&message.address.uri,&child(&message.address.pointer,field),&[],"ASYNCAPI_MESSAGE_METADATA",Classification::Omitted,format!("Message {field} requires an explicit HTTP mapping; original remains available for review"),Some(value));
        }
    }
    let Some(payload) = message.value.get("payload") else {
        return Ok(Payload::default());
    };
    if replace_payload {
        trace.record(
            sources,
            &message.address.uri,
            &child(&message.address.pointer, "payload"),
            &[],
            "ASYNCAPI_PAYLOAD_REPLACED",
            Classification::Omitted,
            "Caller-authored HTTP schema replaces this operation's source payload. Review equivalence; original payload remains in source evidence.",
            Some(payload),
        );
        return Ok(Payload {
            schema: None,
            declared: true,
        });
    }
    let mut payload = payload_wrapper(
        sources,
        Located {
            value: payload.clone(),
            address: Address {
                uri: message.address.uri.clone(),
                pointer: child(&message.address.pointer, "payload"),
            },
        },
    )?;
    // AsyncAPI 3 places schemaFormat on a Multi Format Schema Object. In 2.x
    // it belongs to the Message Object. Never treat the wrapper as a schema.
    let format = if payload.value.get("schemaFormat").is_some() {
        let format = payload.value["schemaFormat"]
            .as_str()
            .ok_or_else(|| {
                ConversionError::new("ASYNCAPI_PAYLOAD_FORMAT", "schemaFormat must be a string")
            })?
            .to_owned();
        let schema = payload.value.get("schema").cloned().ok_or_else(|| {
            ConversionError::new(
                "ASYNCAPI_PAYLOAD_FORMAT",
                "Multi Format Schema Object requires schema",
            )
        })?;
        payload.value = schema;
        payload.address.pointer = child(&payload.address.pointer, "schema");
        Some(format)
    } else {
        message
            .value
            .get("schemaFormat")
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    ConversionError::new("ASYNCAPI_PAYLOAD_FORMAT", "schemaFormat must be a string")
                })
            })
            .transpose()?
    };
    if let Some(format) = format.as_deref()
        && schema_format(format).is_none()
    {
        trace.record(sources,&message.address.uri,&child(&message.address.pointer,"payload"),&[],"ASYNCAPI_PAYLOAD_FORMAT",Classification::Blocked,format!("Payload schema format {format} requires its matching schema adapter. Its source is retained; no JSON Schema constraints are invented."),Some(&json!(format)));
        return Ok(Payload {
            schema: None,
            declared: true,
        });
    }
    let dialect = schema_dialect(format.as_deref()).ok_or_else(|| {
        ConversionError::new(
            "ASYNCAPI_PAYLOAD_FORMAT",
            "Payload format has no supported schema dialect",
        )
    })?;
    if dialect == octri_parser::SchemaDialect::Draft7 {
        trace.record(sources, &payload.address.uri, &payload.address.pointer, &[], "ASYNCAPI_DRAFT7_PRESERVED", Classification::Transformed,
            "Draft 7 schema and reference semantics retained with an explicit dialect in OpenAPI 3.1+. Older output dialects still require review of incompatible keywords.", None);
    }
    Ok(Payload {
        schema: Some(schemas.import_with_dialect(payload, dialect)?),
        declared: true,
    })
}

pub(in crate::adapters) fn schema_dialect(
    format: Option<&str>,
) -> Option<octri_parser::SchemaDialect> {
    match format
        .map(schema_format)
        .unwrap_or(Some(PayloadDialect::Draft7))?
    {
        PayloadDialect::Draft7 => Some(octri_parser::SchemaDialect::Draft7),
        PayloadDialect::Draft201909 => Some(octri_parser::SchemaDialect::Draft201909),
        PayloadDialect::Modern => Some(octri_parser::SchemaDialect::Draft202012),
    }
}

/// Resolve only wrapper references. Ordinary JSON Schema references belong to
/// the parser, including logical resource IDs that are not retrieval URLs.
fn payload_wrapper(sources: &mut Sources, original: Located) -> Result<Located> {
    let mut candidate = original.clone();
    let mut seen = std::collections::BTreeSet::new();
    while let Some(reference) = candidate.value.get("$ref").and_then(Value::as_str) {
        if !seen.insert(candidate.address.clone()) || seen.len() > sources.limits.max_depth {
            break;
        }
        let Ok(target) = references::address(&candidate.address.uri, reference) else {
            break;
        };
        if !sources.files.contains_key(&target.uri) {
            break;
        }
        let document = sources.parse(&target.uri, false)?;
        let Some(value) = document.pointer(&target.pointer) else {
            break;
        };
        candidate = Located {
            value: value.clone(),
            address: target,
        };
        if candidate.value.get("schemaFormat").is_some() {
            return Ok(candidate);
        }
    }
    Ok(original)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PayloadDialect {
    Draft7,
    Draft201909,
    Modern,
}

fn schema_format(format: &str) -> Option<PayloadDialect> {
    let mut parts = format.split(';');
    let media_type = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let mut version = None;
    for parameter in parts {
        let (name, value) = parameter.trim().split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("version") || version.is_some() {
            return None;
        }
        version = Some(value.trim().trim_matches('"'));
    }
    match media_type.as_str() {
        "application/schema+json" | "application/schema+yaml" => match version {
            Some("draft-07") => Some(PayloadDialect::Draft7),
            Some("draft-2019-09") => Some(PayloadDialect::Draft201909),
            Some("draft-2020-12") => Some(PayloadDialect::Modern),
            _ => None,
        },
        "application/vnd.aai.asyncapi"
        | "application/vnd.aai.asyncapi+json"
        | "application/vnd.aai.asyncapi+yaml" => version
            .filter(|version| {
                matches!(
                    *version,
                    "2.0.0" | "2.1.0" | "2.2.0" | "2.3.0" | "2.4.0" | "2.5.0" | "2.6.0" | "3.0.0"
                )
            })
            .map(|_| PayloadDialect::Draft7),
        _ => None,
    }
}

pub(super) fn combine(payloads: Vec<Payload>) -> Payload {
    let mut values = Vec::new();
    let mut declared = false;
    for payload in payloads {
        declared |= payload.declared;
        if let Some(schema) = payload.schema
            && !values.contains(&schema)
        {
            values.push(schema);
        }
    }
    let schema = match values.as_slice() {
        [] => None,
        [one] => Some(one.clone()),
        _ => Some(json!({"oneOf":values})),
    };
    Payload { schema, declared }
}
