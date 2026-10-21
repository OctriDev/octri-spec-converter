//! One bounded JSON protocol for every adapter. ABI and report versions differ.

use crate::{
    AuditError, AuditReport, AuditRequest, MAX_ENVELOPE_BYTES, Result, audit, catalogue, export,
    fixes,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
enum Call {
    Audit {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
        request: Box<AuditRequest>,
    },
    Fix {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
        request: Box<fixes::FixRequest>,
    },
    Compare {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
        before: Box<AuditReport>,
        after: Box<AuditReport>,
    },
    Export {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
        report: Box<AuditReport>,
        format: String,
    },
    Rules {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
    },
    LegacyFix {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
        request: Box<crate::legacy::LegacyFixRequest>,
    },
    Bundle {
        #[serde(rename = "abiVersion")]
        abi_version: u32,
        request: Box<AuditRequest>,
    },
}

/// Safe Rust dispatcher; FFI only handles allocation and borrowed byte ownership.
pub fn dispatch(input: &str) -> String {
    encode(dispatch_result(input))
}

fn dispatch_result(input: &str) -> Result<Value> {
    if input.len() > MAX_ENVELOPE_BYTES {
        return Err(AuditError::new(
            "LIMIT_REQUEST_BYTES",
            "Binding envelope exceeds 128 MiB",
        ));
    }
    let call: Call = octri_parser::wire::decode_json("", input)
        .map_err(|error| AuditError::new("INVALID_REQUEST", error.to_string()))?;
    let version = match &call {
        Call::Audit { abi_version, .. }
        | Call::Fix { abi_version, .. }
        | Call::Compare { abi_version, .. }
        | Call::Export { abi_version, .. }
        | Call::Rules { abi_version }
        | Call::LegacyFix { abi_version, .. }
        | Call::Bundle { abi_version, .. } => *abi_version,
    };
    if version != 1 {
        return Err(AuditError::new(
            "ABI_VERSION",
            "Expected auditor ABI version 1",
        ));
    }
    match call {
        Call::Audit { request, .. } => value(audit(&request)?),
        Call::Fix { request, .. } => value(fixes::apply(&request)?),
        Call::Compare { before, after, .. } => value(export::compare(&before, &after)?),
        Call::Rules { .. } => value(catalogue::RULES),
        Call::LegacyFix { request, .. } => value(crate::legacy::fix(*request)?),
        Call::Bundle { request, .. } => value(crate::bundle(&request)?),
        Call::Export { report, format, .. } => match format.as_str() {
            "html" => Ok(json!(export::html(&report))),
            "sarif" => Ok(export::sarif(&report)),
            "annotations" => Ok(json!(export::annotations(&report))),
            "baseline" => value(export::baseline_json(&report)),
            "json" => value(report),
            _ => Err(AuditError::new(
                "INVALID_FORMAT",
                "Supported formats: json, html, sarif, annotations, baseline",
            )),
        },
    }
}

fn value(value: impl Serialize) -> Result<Value> {
    serde_json::to_value(value).map_err(|error| AuditError::new("SERIALIZATION", error.to_string()))
}
pub fn encode<T: Serialize>(result: Result<T>) -> String {
    match serde_json::to_string(&result) {
        Ok(output) if output.len()<=MAX_ENVELOPE_BYTES=>output,
        Ok(_)=>"{\"Err\":{\"code\":\"LIMIT_OUTPUT_BYTES\",\"message\":\"Auditor result exceeds 128 MiB\",\"diagnostics\":[]}}".into(),
        Err(_)=>"{\"Err\":{\"code\":\"SERIALIZATION\",\"message\":\"Could not serialize auditor result\",\"diagnostics\":[]}}".into(),
    }
}
