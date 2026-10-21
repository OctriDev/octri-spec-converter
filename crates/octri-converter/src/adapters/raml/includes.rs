//! RAML includes are virtual dependencies, not filesystem or network reads.
use crate::source::{Sources, child};
use crate::{ConversionError, Result};
use octri_parser::Address;
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn expand(sources: &mut Sources) -> Result<Value> {
    let uri = sources.root.clone();
    expand_uri(sources, &uri, "")
}

pub(super) fn expand_uri(sources: &mut Sources, uri: &str, output: &str) -> Result<Value> {
    let root = sources.parse(uri, true)?;
    let mut active = BTreeSet::new();
    let mut nodes = 0;
    expand_at(
        root,
        Address {
            uri: uri.into(),
            pointer: String::new(),
        },
        output,
        sources,
        &mut active,
        &mut nodes,
        0,
    )
}

fn expand_at(
    value: Value,
    address: Address,
    output: &str,
    sources: &mut Sources,
    active: &mut BTreeSet<Address>,
    nodes: &mut usize,
    depth: usize,
) -> Result<Value> {
    *nodes += 1;
    if *nodes > sources.limits.max_nodes || depth > sources.limits.max_depth {
        return Err(ConversionError::new(
            "RAML_EXPANSION_LIMIT",
            "Expanded RAML exceeds node or depth limit",
        ));
    }
    if let Some(path) = sources.includes.get(&address).cloned() {
        if !active.insert(address.clone()) {
            return Err(ConversionError::new(
                "RAML_INCLUDE_CYCLE",
                "RAML !include graph is cyclic",
            ));
        }
        if let Some(value) = super::schema_fragments::include(sources, &address, &path)
            .map_err(|error| super::schema_fragments::located(error, &address, sources))?
        {
            sources.origins.insert(output.into(), address.clone());
            active.remove(&address);
            return Ok(value);
        }
        let uri = sources.relative(&address.uri, &path)?;
        let text = sources.text(&uri)?;
        let is_literal = super::schema_use::document_text(text).starts_with('<')
            || matches!(uri.rsplit('.').next(), Some("txt" | "md" | "xml" | "html"));
        let value = if is_literal {
            Value::String(text.into())
        } else {
            sources.parse(&uri, true)?
        };
        let result = expand_at(
            value,
            Address {
                uri,
                pointer: String::new(),
            },
            output,
            sources,
            active,
            nodes,
            depth + 1,
        )?;
        active.remove(&address);
        return Ok(result);
    }
    sources.origins.insert(output.into(), address.clone());
    match value {
        Value::Object(fields) => {
            let mut result = serde_json::Map::new();
            for (key, value) in fields {
                result.insert(
                    key.clone(),
                    expand_at(
                        value,
                        Address {
                            uri: address.uri.clone(),
                            pointer: child(&address.pointer, &key),
                        },
                        &child(output, &key),
                        sources,
                        active,
                        nodes,
                        depth + 1,
                    )?,
                );
            }
            Ok(Value::Object(result))
        }
        Value::Array(values) => {
            let mut result = Vec::new();
            for (index, value) in values.into_iter().enumerate() {
                let key = index.to_string();
                result.push(expand_at(
                    value,
                    Address {
                        uri: address.uri.clone(),
                        pointer: child(&address.pointer, &key),
                    },
                    &child(output, &key),
                    sources,
                    active,
                    nodes,
                    depth + 1,
                )?);
            }
            Ok(Value::Array(result))
        }
        scalar => Ok(scalar),
    }
}
