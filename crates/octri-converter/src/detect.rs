//! Format detection identifies declarations, not merely a matching filename.

use crate::source::Sources;
use crate::{ConversionError, InputFormat, Result};
use serde_json::Value;

pub(crate) fn detect(sources: &mut Sources, selected: InputFormat) -> Result<InputFormat> {
    let uri = sources.root.clone();
    let text = sources
        .text(&uri)?
        .trim_start_matches('\u{feff}')
        .trim_start();
    let first = text.lines().next().unwrap_or("");
    let textual = if matches!(
        first.trim(),
        "#%RAML 1.0" | "#%RAML 0.8" | "#%RAML 1.0 Overlay" | "#%RAML 1.0 Extension"
    ) {
        Some(InputFormat::Raml)
    } else if text.starts_with("curl.exe ")
        || text.starts_with("curl ")
        || text.starts_with("curl\t")
        || text.starts_with("curl\\")
    {
        Some(InputFormat::Curl)
    } else if text.starts_with('<') {
        Some(InputFormat::Wsdl)
    } else if text.starts_with("syntax ")
        || text.starts_with("syntax=")
        || text.starts_with("edition ")
        || uri.ends_with(".proto")
    {
        Some(InputFormat::Protobuf)
    } else if uri.ends_with(".graphql")
        || uri.ends_with(".gql")
        || text.starts_with("type ")
        || text.starts_with("schema ")
        || text.starts_with("scalar ")
        || text.starts_with("directive ")
    {
        Some(InputFormat::Graphql)
    } else {
        None
    };
    let detected = if let Some(kind) = textual {
        kind
    } else if matches!(
        selected,
        InputFormat::Graphql | InputFormat::Protobuf | InputFormat::Curl
    ) && !text.starts_with(['{', '['])
    {
        // Pasted text has no meaningful filename. Comments, descriptions and
        // quoted command names need not match the auto-detection prefixes.
        // The selected adapter still validates its complete language syntax.
        selected
    } else {
        let document = sources.parse(&uri, false)?;
        detect_object(&document)?
    };
    if selected != InputFormat::Auto && selected != detected {
        return Err(ConversionError::new(
            "FORMAT_MISMATCH",
            format!("Selected {selected:?}, but the source declares {detected:?}"),
        ));
    }
    Ok(detected)
}

fn detect_object(document: &Value) -> Result<InputFormat> {
    let mut matches = Vec::new();
    if document.get("openapi").is_some() {
        matches.push(InputFormat::Openapi);
    }
    if document.get("swagger").is_some() || document.get("swaggerVersion").is_some() {
        matches.push(InputFormat::Swagger);
    }
    if document.get("$kind").and_then(Value::as_str) == Some("collection")
        || document
            .pointer("/info/schema")
            .and_then(Value::as_str)
            .is_some_and(|schema| schema.contains("postman") && schema.contains("collection"))
    {
        matches.push(InputFormat::Postman);
    }
    if document.get("_type").and_then(Value::as_str) == Some("export")
        || document
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| {
                kind.starts_with("collection.insomnia.rest/")
                    || kind.starts_with("spec.insomnia.rest/")
            })
    {
        matches.push(InputFormat::Insomnia);
    }
    if document.pointer("/log/entries").is_some() {
        matches.push(InputFormat::Har);
    }
    if document.get("asyncapi").is_some() {
        matches.push(InputFormat::Asyncapi);
    }
    if document.get("__schema").is_some() || document.pointer("/data/__schema").is_some() {
        matches.push(InputFormat::Graphql);
    }
    if document
        .get("file")
        .and_then(Value::as_array)
        .is_some_and(|files| {
            files
                .iter()
                .any(|file| file.get("messageType").is_some() || file.get("service").is_some())
        })
    {
        matches.push(InputFormat::Protobuf);
    }
    match matches.as_slice() {
        [kind] => Ok(*kind),
        [] => Err(ConversionError::new(
            "UNRECOGNIZED_FORMAT",
            "No supported format declaration found. Supply the original specification, not a documentation page.",
        )),
        _ => Err(ConversionError::new(
            "AMBIGUOUS_FORMAT",
            format!("Conflicting format declarations: {matches:?}"),
        )),
    }
}
