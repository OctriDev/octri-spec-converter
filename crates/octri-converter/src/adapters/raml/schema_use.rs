//! RAML schema documents may describe bodies, but cannot become RAML types.
use crate::source::{Sources, child};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

/// Recognize an embedded document header without changing its original bytes.
/// A BOM is a document marker, never part of a RAML type expression.
pub(super) fn document_text(raw: &str) -> &str {
    let text = raw.trim();
    let document = text.strip_prefix('\u{feff}').unwrap_or(text).trim_start();
    if document.starts_with(['{', '<']) {
        document
    } else {
        text
    }
}

#[derive(Clone, Copy)]
pub(super) enum SchemaKind {
    Json,
    Xml,
}

#[derive(Clone, Copy)]
pub(super) enum SchemaUse {
    Declaration,
    Body { json: bool, xml: bool },
    RamlType,
}

impl SchemaUse {
    pub fn permits(self, kind: SchemaKind) -> bool {
        match (self, kind) {
            (Self::Declaration, _) => true,
            (Self::Body { json, .. }, SchemaKind::Json) => json,
            (Self::Body { xml, .. }, SchemaKind::Xml) => xml,
            _ => false,
        }
    }
}

pub(super) fn external_names(root: &Value, sources: &Sources) -> BTreeMap<String, SchemaKind> {
    let mut external = BTreeMap::new();
    let mut aliases: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in root
        .get("types")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let pointer = child("/types", name);
        if let Some(kind) = kind(value, &pointer, sources, &BTreeMap::new()) {
            external.insert(name.clone(), kind);
        } else if let Some(base) = base_name(value) {
            aliases.entry(base.into()).or_default().push(name.clone());
        }
    }
    let mut pending: VecDeque<_> = external
        .iter()
        .map(|(name, kind)| (name.clone(), *kind))
        .collect();
    while let Some((name, kind)) = pending.pop_front() {
        for alias in aliases.remove(&name).unwrap_or_default() {
            if external.insert(alias.clone(), kind).is_none() {
                pending.push_back((alias, kind));
            }
        }
    }
    external
}

pub(super) fn kind(
    value: &Value,
    pointer: &str,
    sources: &Sources,
    names: &BTreeMap<String, SchemaKind>,
) -> Option<SchemaKind> {
    let mut value = value;
    let mut pointer = pointer.to_owned();
    loop {
        if let Some(kind) = sources
            .origins
            .get(&pointer)
            .and_then(|site| super::schema_fragments::kind(site, sources))
        {
            return Some(kind);
        }
        if value.get("$schema").is_some()
            || sources
                .origins
                .get(&pointer)
                .is_some_and(|origin| origin.uri != sources.root && origin.uri.ends_with(".json"))
        {
            return Some(SchemaKind::Json);
        }
        if let Some(text) = value.as_str() {
            let text = document_text(text);
            return if text.starts_with('{') {
                Some(SchemaKind::Json)
            } else if text.starts_with('<') {
                Some(SchemaKind::Xml)
            } else {
                names.get(text).copied()
            };
        }
        let (key, base) = base(value)?;
        value = base;
        pointer = child(&pointer, key);
    }
}

fn base_name(mut value: &Value) -> Option<&str> {
    loop {
        if let Some(name) = value.as_str() {
            return Some(name.trim());
        }
        value = base(value)?.1;
    }
}

fn base(value: &Value) -> Option<(&'static str, &Value)> {
    value
        .get("type")
        .map(|value| ("type", value))
        .or_else(|| value.get("schema").map(|value| ("schema", value)))
}
