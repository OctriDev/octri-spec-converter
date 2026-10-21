//! Defaults for the returned working document. Source snapshots stay untouched.
use serde_json::Value;

pub(crate) fn needs_info_version(document: &Value) -> bool {
    document
        .get("info")
        .and_then(Value::as_object)
        .is_some_and(|info| !info.get("version").is_some_and(Value::is_string))
}

pub(crate) fn info_version(document: &mut Value) {
    if let Some(info) = document.get_mut("info").and_then(Value::as_object_mut)
        && !info.get("version").is_some_and(Value::is_string)
    {
        info.insert("version".into(), Value::String("0.1.0".into()));
    }
}
