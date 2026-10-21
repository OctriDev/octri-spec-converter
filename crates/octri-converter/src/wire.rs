//! Versioned transport envelope shared by Rust, native consumers and WASM.
use crate::{ConversionError, ConversionReport, ConversionRequest, MAX_ENVELOPE_BYTES, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
enum Call {
    Bundle {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
        request: Box<ConversionRequest>,
    },
    Compare {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
        before: Box<ConversionReport>,
        after: Box<ConversionReport>,
    },
    Export {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
        report: Box<ConversionReport>,
        format: String,
    },
    Convert {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
        request: Box<ConversionRequest>,
    },
    Review {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
        request: Box<ConversionRequest>,
    },
    Detect {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
        request: Box<ConversionRequest>,
    },
    Formats {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
    },
    Batch {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
        requests: Vec<ConversionRequest>,
    },
}

pub fn dispatch(input: &str) -> String {
    encode(dispatch_result(input))
}

fn dispatch_result(input: &str) -> Result<Value> {
    if input.len() > MAX_ENVELOPE_BYTES {
        return Err(ConversionError::new(
            "LIMIT_REQUEST_BYTES",
            "Binding request exceeds 128 MiB",
        ));
    }
    let call: Call = octri_parser::wire::decode_json("", input)
        .map_err(|error| ConversionError::new("INVALID_REQUEST", error.to_string()))?;
    let version = match &call {
        Call::Convert { abi_version, .. }
        | Call::Review { abi_version, .. }
        | Call::Detect { abi_version, .. }
        | Call::Formats { abi_version }
        | Call::Bundle { abi_version, .. }
        | Call::Compare { abi_version, .. }
        | Call::Export { abi_version, .. }
        | Call::Batch { abi_version, .. } => *abi_version,
    };
    if version != 1 {
        return Err(ConversionError::new(
            "ABI_VERSION",
            "Expected converter ABI version 1",
        ));
    }
    match call {
        Call::Bundle { request, .. } => value(crate::artifacts::bundle(&request)?),
        Call::Compare { before, after, .. } => crate::artifacts::compare(&before, &after),
        Call::Export { report, format, .. } => crate::artifacts::export(&report, &format),
        Call::Convert { request, .. } | Call::Review { request, .. } => {
            value(crate::convert(&request)?)
        }
        Call::Detect { request, .. } => value(crate::detect(&request)?),
        Call::Formats { .. } => Ok(crate::formats()),
        Call::Batch { requests, .. } => {
            if requests.is_empty() || requests.len() > 100 {
                return Err(ConversionError::new(
                    "BATCH_SIZE",
                    "Supply between 1 and 100 requests",
                ));
            }
            let total = requests.iter().try_fold(0usize, |total, request| {
                request.files.values().try_fold(
                    total.saturating_add(request.input.content.as_ref().map_or(0, String::len)),
                    |total, text| total.checked_add(text.len()),
                )
            });
            if total.is_none_or(|total| total > 20 * 1024 * 1024) {
                return Err(ConversionError::new(
                    "BATCH_INPUT_LIMIT",
                    "Combined batch sources exceed 20 MiB; split the batch",
                ));
            }
            let mut results = Vec::new();
            let mut bytes = 0usize;
            for request in &requests {
                let result = value(crate::convert(request))?;
                let size = serde_json::to_vec(&result)
                    .map_err(|error| ConversionError::new("SERIALIZATION", error.to_string()))?
                    .len();
                bytes = bytes.saturating_add(size);
                if bytes > MAX_ENVELOPE_BYTES - 1024 * 1024 {
                    return Err(ConversionError::new(
                        "BATCH_OUTPUT_LIMIT",
                        "Combined reports exceed transport budget; split the batch",
                    ));
                }
                results.push(result);
            }
            Ok(json!(results))
        }
    }
}

fn value(value: impl Serialize) -> Result<Value> {
    serde_json::to_value(value)
        .map_err(|error| ConversionError::new("SERIALIZATION", error.to_string()))
}

pub fn encode<T: Serialize>(result: Result<T>) -> String {
    match serde_json::to_string(&result) {
        Ok(output) if output.len()<=MAX_ENVELOPE_BYTES=>output,
        Ok(_)=>json!({"Err":{"code":"LIMIT_OUTPUT_BYTES","message":"Converter result exceeds 128 MiB","diagnostics":[]}}).to_string(),
        Err(_)=>json!({"Err":{"code":"SERIALIZATION","message":"Could not serialize converter result","diagnostics":[]}}).to_string(),
    }
}
