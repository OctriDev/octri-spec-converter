//! HAR 1.2 observed transactions. Captured metadata never becomes validation rules.
use super::{
    common::unknown_fields,
    http::{self, Auth, Header, Observation, Payload, Response},
};
use crate::{Classification, Config, ConversionError, Result};
use crate::{
    source::{Sources, child},
    trace::Trace,
};
use base64::Engine;
use serde_json::{Map, Value, json};

pub(crate) fn convert(sources: &mut Sources, config: &Config, trace: &mut Trace) -> Result<Value> {
    let uri = sources.root.clone();
    let document = sources.parse(&uri, false)?;
    if document.pointer("/log/version").and_then(Value::as_str) != Some("1.2") {
        return Err(ConversionError::new("HAR_VERSION", "Expected HAR 1.2 log"));
    }
    let entries = document
        .pointer("/log/entries")
        .and_then(Value::as_array)
        .ok_or_else(|| ConversionError::new("HAR_ENTRIES", "HAR log needs entries array"))?;
    let mut builder = http::Builder::new("Converted HTTP archive", sources, config, trace);
    for (index, entry) in entries.iter().enumerate() {
        let at = child("/log/entries", &index.to_string());
        let request = &entry["request"];
        let response = &entry["response"];
        let url = request["url"]
            .as_str()
            .ok_or_else(|| ConversionError::new("HAR_URL", "HAR request needs URL"))?
            .to_owned();
        let method = request["method"]
            .as_str()
            .ok_or_else(|| ConversionError::new("HAR_METHOD", "HAR request needs method"))?
            .to_owned();
        let headers = headers(request.get("headers"), &child(&at, "request/headers"))?;
        let body = post_data(
            request.get("postData"),
            &child(&child(&at, "request"), "postData"),
            sources,
        )?;
        let status=response["status"].as_u64().filter(|status|(100..=599).contains(status)).ok_or_else(||ConversionError::new("HAR_STATUS","HAR response needs a captured HTTP status in 100..599; network failures are not API responses"))?;
        let response_headers = headers_from(
            response.get("headers"),
            &child(&child(&at, "response"), "headers"),
        )?;
        let content = &response["content"];
        let media = content["mimeType"].as_str();
        let text = content["text"].as_str().unwrap_or("");
        let content_pointer = child(&child(&at, "response"), "content");
        let payload = match content.get("encoding").and_then(Value::as_str) {
            None => http::example_payload(text, media, &content_pointer, sources)?,
            Some("base64") => {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(text)
                    .map_err(|_| {
                        ConversionError::new("HAR_ENCODING", "Invalid base64 response content")
                    })?;
                if decoded.len() > config.limits.max_document_bytes {
                    return Err(ConversionError::new(
                        "LIMIT_INPUT",
                        "Decoded HAR response exceeds byte limit",
                    ));
                }
                match String::from_utf8(decoded) {
                    Ok(text)
                        if media.is_some_and(|media| {
                            media.starts_with("text/")
                                || media.contains("json")
                                || media.contains("xml")
                        }) =>
                    {
                        http::example_payload(&text, media, &content_pointer, sources)?
                    }
                    _ => Some(Payload {
                        media: media.unwrap_or("application/octet-stream").into(),
                        schema: json!({"type":"string","format":"binary"}),
                        example: None,
                        encoding: None,
                        pointer: content_pointer.clone(),
                        inferred: true,
                    }),
                }
            }
            Some(encoding) => {
                return Err(ConversionError::new(
                    "HAR_ENCODING",
                    format!("Unknown HAR content encoding {encoding}"),
                ));
            }
        };
        if text.is_empty() && content["size"].as_u64().is_some_and(|size| size > 0) {
            builder.record(
                &content_pointer,
                &[],
                "RESPONSE_BODY_NOT_CAPTURED",
                Classification::Inferred,
                "HAR records a body size but omits captured content; no response schema inferred",
                Some(content),
            );
        }
        for (value, pointer) in [
            (request, child(&at, "request")),
            (response, child(&at, "response")),
        ] {
            if let Some(cookies) = value
                .get("cookies")
                .filter(|value| value.as_array().is_some_and(|cookies| !cookies.is_empty()))
            {
                builder.record(&child(&pointer,"cookies"),&[],"OBSERVED_COOKIES",Classification::Omitted,"Observed cookie values remain in original HAR; authentication and Set-Cookie contracts require authored mappings",Some(cookies));
            }
        }
        builder.add(Observation {
            pointer: at.clone(),
            name: format!("{method} {}", url.split('?').next().unwrap_or(&url)),
            description: entry["comment"].as_str().map(str::to_owned),
            method,
            url,
            headers,
            path_variables: Default::default(),
            body,
            responses: vec![Response {
                status: status.to_string(),
                description: response["statusText"]
                    .as_str()
                    .filter(|text| !text.is_empty())
                    .unwrap_or("Captured response")
                    .into(),
                headers: response_headers,
                payload,
                pointer: child(&at, "response"),
            }],
            tags: entry["pageref"]
                .as_str()
                .map(|tag| vec![tag.into()])
                .unwrap_or_default(),
            auth: Auth::Unknown,
        })?;
        unknown_fields(
            request,
            &[
                "method",
                "url",
                "httpVersion",
                "cookies",
                "headers",
                "queryString",
                "postData",
                "headersSize",
                "bodySize",
                "comment",
            ],
            sources,
            &uri,
            &child(&at, "request"),
            builder.trace,
        );
        unknown_fields(
            response,
            &[
                "status",
                "statusText",
                "httpVersion",
                "cookies",
                "headers",
                "content",
                "redirectURL",
                "headersSize",
                "bodySize",
                "comment",
            ],
            sources,
            &uri,
            &child(&at, "response"),
            builder.trace,
        );
    }
    builder.record("/log",&[],"HAR_CAPTURE_METADATA",Classification::Transformed,"Capture timing, cache, network and browser metadata retained in original sources; OpenAPI describes HTTP contracts",None);
    Ok(builder.finish())
}

fn headers(value: Option<&Value>, pointer: &str) -> Result<Vec<Header>> {
    headers_from(value, pointer)
}
fn headers_from(value: Option<&Value>, pointer: &str) -> Result<Vec<Header>> {
    let entries = value
        .and_then(Value::as_array)
        .ok_or_else(|| ConversionError::new("HAR_HEADERS", "HAR headers must be an array"))?;
    entries
        .iter()
        .enumerate()
        .map(|(index, header)| {
            Ok(Header {
                name: header["name"]
                    .as_str()
                    .ok_or_else(|| ConversionError::new("HAR_HEADER", "Header needs name"))?
                    .into(),
                value: header["value"]
                    .as_str()
                    .ok_or_else(|| ConversionError::new("HAR_HEADER", "Header needs value"))?
                    .into(),
                pointer: child(pointer, &index.to_string()),
            })
        })
        .collect()
}

fn post_data(value: Option<&Value>, pointer: &str, sources: &Sources) -> Result<Option<Payload>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let media = value["mimeType"]
        .as_str()
        .ok_or_else(|| ConversionError::new("HAR_BODY", "HAR postData needs mimeType"))?;
    if let Some(text) = value["text"].as_str() {
        return http::example_payload(text, Some(media), pointer, sources);
    }
    let Some(params) = value["params"].as_array() else {
        return Ok(None);
    };
    let mut properties = Map::new();
    let mut examples = Map::new();
    let mut encoding = Map::new();
    for field in params {
        let name = field["name"]
            .as_str()
            .ok_or_else(|| ConversionError::new("HAR_BODY", "HAR body parameter needs name"))?;
        let schema = if field.get("fileName").is_some() {
            json!({"type":"string","format":"binary"})
        } else {
            json!({"type":"string"})
        };
        if properties.contains_key(name) {
            return Err(ConversionError::new(
                "HAR_BODY",
                "Repeated form fields need an explicit combined array mapping",
            ));
        }
        properties.insert(name.into(), schema);
        if let Some(value) = field.get("value") {
            examples.insert(name.into(), value.clone());
        }
        if let Some(content_type) = field.get("contentType") {
            encoding.insert(name.into(), json!({"contentType":content_type}));
        }
    }
    Ok(Some(Payload {
        media: media.into(),
        schema: json!({"type":"object","properties":properties}),
        example: if examples.is_empty() {
            None
        } else {
            Some(Value::Object(examples))
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
