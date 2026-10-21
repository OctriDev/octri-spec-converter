//! Credential rules are retained, never guessed. An explicit choice binds the
//! request and inherited auth scope to the exact original collection bytes.

use super::{Auth, Context, InheritedAuth};
use crate::adapters::common::object;
use crate::source::child;
use crate::{
    Classification, ConversionError, CredentialChoice, CredentialSelectionRequest, Result,
};
use serde_json::{Value, json};

pub(super) fn inherited_auth(
    context: &mut Context<'_>,
    inherited: &InheritedAuth,
    request_pointer: &str,
    request_name: &str,
) -> Result<Auth> {
    if context.trace.truncated {
        return Ok(Auth::Unknown);
    }
    let Some(sets) = inherited.value.as_array() else {
        return context.auth(Some(inherited.value.as_ref()), &inherited.pointer);
    };
    let authored = context.config.credential_selections.get(request_pointer);
    let unconditional = sets.first().is_some_and(|first| {
        first
            .get("rules")
            .is_none_or(|rules| rules.as_array().is_some_and(Vec::is_empty))
    });
    if authored.is_none() && unconditional {
        let first = &sets[0];
        let pointer = child(&inherited.pointer, "0");
        if !declared_fields(context, first, &pointer)? {
            return Ok(Auth::Unknown);
        }
        context.record(&pointer, "POSTMAN_AUTH_SET_SELECTED", Classification::Transformed,
            "Selected the first unconditional credential set in source order; later sets cannot override it", first.get("id"));
        return context.auth(Some(first), &pointer);
    }

    let source_sha256 = context
        .trace
        .source_sha256(&context.sources.root)
        .ok_or_else(|| {
            ConversionError::new("POSTMAN_COLLECTION", "Original collection hash is missing")
        })?
        .to_owned();
    let selected = authored
        .filter(|selection| {
            selection.source_uri == context.sources.root
                && selection.source_sha256 == source_sha256
                && selection.auth_pointer == inherited.pointer
        })
        .and_then(|selection| usize::try_from(selection.credential_index).ok())
        .filter(|index| sets.get(*index).is_some());
    let described = context
        .trace
        .describe_credentials(CredentialSelectionRequest {
            name: request_name.into(),
            request: context
                .sources
                .location(&context.sources.root, request_pointer),
            auth_source: context
                .sources
                .location(&context.sources.root, &inherited.pointer),
            source_sha256,
            choices: sets
                .iter()
                .enumerate()
                .map(|(index, value)| CredentialChoice {
                    index,
                    id: value.get("id").and_then(Value::as_str).map(str::to_owned),
                    name: value.get("name").and_then(Value::as_str).map(str::to_owned),
                    auth_type: value
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .into(),
                })
                .collect(),
            selected,
        });
    if !described {
        return Ok(Auth::Unknown);
    }
    if authored.is_some() && selected.is_none() {
        context.record(request_pointer, "POSTMAN_AUTH_SELECTION_STALE", Classification::Blocked,
            "Credential choice does not match the current collection bytes, inherited auth scope or available set. Select credentials again from this report.", None);
        return Ok(Auth::Unknown);
    }
    let Some(index) = selected else {
        // Retain fail-closed field coverage on the first candidate even when
        // its predicate cannot be evaluated. Explicit choices validate their
        // chosen set instead of interpreting any selection predicates.
        if let Some(first) = sets.first() {
            declared_fields(context, first, &child(&inherited.pointer, "0"))?;
        }
        context.record(&inherited.pointer, "POSTMAN_AUTH_RULES", Classification::Blocked,
            "Choose credentials for this request. Conditional URL matching is not defined by the source schema and cannot be guessed or downgraded to anonymous access.", None);
        return Ok(Auth::Unknown);
    };
    let pointer = child(&inherited.pointer, &index.to_string());
    let selected_set = &sets[index];
    if !declared_fields(context, selected_set, &pointer)? {
        return Ok(Auth::Unknown);
    }
    context.record(&pointer, "POSTMAN_AUTH_SET_AUTHORED", Classification::Inferred,
        format!("Caller selected credential set {} for request {request_pointer}. Original selection rules remain in source and were not evaluated; review the resulting security contract.", index + 1),
        Some(&json!({"request":request_pointer,"credentialIndex":index})));
    context.auth(Some(selected_set), &pointer)
}

fn declared_fields(context: &mut Context<'_>, value: &Value, pointer: &str) -> Result<bool> {
    let fields = object(value, "Postman credential set")?;
    let mut supported = true;
    for (key, value) in fields {
        if !matches!(
            key.as_str(),
            "id" | "name" | "description" | "rules" | "credentials" | "type"
        ) {
            context.record(&child(pointer, key), "POSTMAN_AUTH_FIELD_UNSUPPORTED", Classification::Blocked,
                format!("Unknown credential field {key} may affect authentication selection; supply a credential set with declared selection semantics"), Some(value));
            supported = false;
        }
    }
    Ok(supported)
}
