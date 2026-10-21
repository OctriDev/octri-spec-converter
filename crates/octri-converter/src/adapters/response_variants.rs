//! Conservative unions of observed responses. No observation becomes required.
use crate::source::hash;
use serde_json::{Map, Value, json};

pub(super) fn merge(previous: &Value, incoming: &Value) -> Value {
    let mut combined = previous.clone();
    if previous["description"] != incoming["description"] {
        let descriptions = combined.as_object_mut().map(|object| {
            object
                .entry("x-octri-observed-descriptions")
                .or_insert_with(|| json!([previous["description"]]))
        });
        if let Some(Value::Array(values)) = descriptions
            && !values.contains(&incoming["description"])
        {
            values.push(incoming["description"].clone());
        }
    }
    for (media, value) in incoming
        .get("content")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if combined.get("content").is_none() {
            combined["content"] = json!({});
        }
        match combined["content"].get_mut(media) {
            Some(existing) => merge_payload(existing, value),
            None => {
                combined["content"][media] = value.clone();
            }
        }
    }
    for (name, value) in incoming
        .get("headers")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if combined.get("headers").is_none() {
            combined["headers"] = json!({});
        }
        let existing = combined["headers"].as_object().and_then(|headers| {
            headers
                .keys()
                .find(|existing| existing.eq_ignore_ascii_case(name))
                .cloned()
        });
        match existing {
            Some(existing) => merge_payload(&mut combined["headers"][existing], value),
            None => {
                combined["headers"][name] = value.clone();
            }
        }
    }
    combined
}

fn merge_payload(existing: &mut Value, incoming: &Value) {
    if let Some(schema) = incoming.get("schema") {
        match existing.get_mut("schema") {
            Some(previous) if !octri_parser::numbers::equivalent(previous, schema) => {
                let mut variants = if previous.as_object().is_some_and(|object| object.len() == 1) {
                    previous
                        .get("anyOf")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_else(|| vec![previous.clone()])
                } else {
                    vec![previous.clone()]
                };
                if !variants
                    .iter()
                    .any(|value| octri_parser::numbers::equivalent(value, schema))
                {
                    variants.push(schema.clone());
                }
                *previous = json!({"anyOf": variants});
            }
            None => existing["schema"] = schema.clone(),
            _ => {}
        }
    }
    let mut examples = Map::new();
    for payload in [&*existing, incoming] {
        if let Some(value) = payload.get("example") {
            examples.insert(
                format!("observed_{}", hash(value.to_string().as_bytes())),
                json!({"value":value}),
            );
        }
        if let Some(values) = payload.get("examples").and_then(Value::as_object) {
            examples.extend(values.clone());
        }
    }
    if !examples.is_empty()
        && let Some(object) = existing.as_object_mut()
    {
        object.remove("example");
        object.insert("examples".into(), Value::Object(examples));
    }
}
