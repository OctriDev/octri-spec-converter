use crate::source::{Sources, child};
use crate::trace::Trace;
use crate::{Classification, ConversionError, Result};
use serde_json::{Map, Value, json};

pub(crate) fn object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| ConversionError::new("INVALID_SOURCE", format!("{name} must be an object")))
}

pub(crate) fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

pub(crate) fn extensions(from: &Map<String, Value>, to: &mut Map<String, Value>) {
    for (key, value) in from {
        if key.starts_with("x-") {
            to.insert(key.clone(), value.clone());
        }
    }
}

pub(crate) fn unknown_fields(
    value: &Value,
    known: &[&str],
    sources: &Sources,
    uri: &str,
    pointer: &str,
    trace: &mut Trace,
) {
    if let Some(map) = value.as_object() {
        for (key, value) in map {
            if !known.contains(&key.as_str()) && !key.starts_with("x-") {
                trace.record(
                    sources,
                    uri,
                    &child(pointer, key),
                    &[],
                    "UNPROCESSED_FIELD",
                    Classification::Omitted,
                    format!("Source field {key} has no declared mapping"),
                    Some(value),
                );
            }
        }
    }
}

pub(crate) fn method(name: &str) -> bool {
    matches!(
        name,
        "get" | "put" | "post" | "delete" | "options" | "head" | "patch" | "trace" | "query"
    )
}

pub(crate) fn operation_id(method: &str, path: &str) -> String {
    let mut id = method.to_lowercase();
    for word in path
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let mut letters = word.chars();
        if let Some(first) = letters.next() {
            id.extend(first.to_uppercase());
            id.push_str(letters.as_str());
        }
    }
    if id.is_empty() {
        "operation".into()
    } else {
        id
    }
}

pub(crate) fn insert_at(document: &mut Value, pointer: &str, value: Value) -> Result<()> {
    if pointer.is_empty() || matches!(pointer, "/openapi" | "/swagger") {
        return Err(ConversionError::new(
            "INVALID_OVERRIDE",
            "Replace a named output property; version selection belongs in targetVersion",
        ));
    }
    let mut tokens = Vec::new();
    for token in pointer
        .strip_prefix('/')
        .ok_or_else(|| ConversionError::new("INVALID_POINTER", "JSON pointers must start with /"))?
        .split('/')
    {
        let mut decoded = String::new();
        let mut chars = token.chars();
        while let Some(ch) = chars.next() {
            if ch == '~' {
                match chars.next() {
                    Some('0') => decoded.push('~'),
                    Some('1') => decoded.push('/'),
                    _ => {
                        return Err(ConversionError::new(
                            "INVALID_POINTER",
                            "Invalid JSON Pointer escape",
                        ));
                    }
                }
            } else {
                decoded.push(ch);
            }
        }
        if decoded == "__proto__" {
            return Err(ConversionError::new("UNSAFE_KEY", "Reserved property name"));
        }
        tokens.push(decoded);
    }
    let Some((last, parents)) = tokens.split_last() else {
        return Err(ConversionError::new(
            "INVALID_POINTER",
            "Missing output property",
        ));
    };
    let mut parent = document;
    for key in parents {
        if parent.is_array() {
            let index = key.parse::<usize>().map_err(|_| {
                ConversionError::new(
                    "INVALID_POINTER",
                    "Array positions must be nonnegative integers",
                )
            })?;
            parent = parent.get_mut(index).ok_or_else(|| {
                ConversionError::new("INVALID_POINTER", "Array position does not exist")
            })?;
        } else {
            let map = parent.as_object_mut().ok_or_else(|| {
                ConversionError::new(
                    "INVALID_POINTER",
                    "Override parent must be an object or array",
                )
            })?;
            parent = map.entry(key.clone()).or_insert_with(|| json!({}));
        }
    }
    if let Some(map) = parent.as_object_mut() {
        map.insert(last.clone(), value);
    } else if let Some(items) = parent.as_array_mut() {
        let index = last
            .parse::<usize>()
            .map_err(|_| ConversionError::new("INVALID_POINTER", "Invalid array position"))?;
        let target = items.get_mut(index).ok_or_else(|| {
            ConversionError::new("INVALID_POINTER", "Array position does not exist")
        })?;
        *target = value;
    } else {
        return Err(ConversionError::new(
            "INVALID_POINTER",
            "Override parent is not a container",
        ));
    }
    Ok(())
}
