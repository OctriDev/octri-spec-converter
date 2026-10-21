//! Bounded literal substitutions. No expression, template tag or script execution.
use crate::{ConversionError, Result};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn expand(
    raw: &str,
    variables: &BTreeMap<String, String>,
    limit: usize,
) -> Result<String> {
    let mut value = raw.to_owned();
    let mut seen = BTreeSet::new();
    for _ in 0..16 {
        if value.contains("{%") || value.contains("{#") {
            return Err(ConversionError::new(
                "TEMPLATE_EXPRESSION",
                "Dynamic template tags require explicit literal values",
            ));
        }
        if !value.contains("{{") {
            return Ok(value);
        }
        if !seen.insert(value.clone()) {
            return Err(ConversionError::new(
                "VARIABLE_CYCLE",
                "Cyclic source variables require explicit values",
            ));
        }
        let mut expanded = String::new();
        let mut tail = value.as_str();
        while let Some((before, rest)) = tail.split_once("{{") {
            let (name, after) = rest.split_once("}}").ok_or_else(|| {
                ConversionError::new("VARIABLE_SYNTAX", "Unclosed variable placeholder")
            })?;
            let name = name.trim().strip_prefix("_.").unwrap_or(name.trim());
            let replacement = variables.get(name).ok_or_else(|| {
                ConversionError::new(
                    "VARIABLE_UNRESOLVED",
                    format!("Supply an explicit value for {name}"),
                )
            })?;
            append(&mut expanded, before, limit)?;
            append(&mut expanded, replacement, limit)?;
            tail = after;
        }
        append(&mut expanded, tail, limit)?;
        value = expanded;
    }
    Err(ConversionError::new(
        "VARIABLE_DEPTH",
        "Variable expansion exceeds 16 passes",
    ))
}

fn append(output: &mut String, value: &str, limit: usize) -> Result<()> {
    if output.len().saturating_add(value.len()) > limit {
        return Err(ConversionError::new(
            "VARIABLE_SIZE",
            "Expanded value exceeds source byte limit",
        ));
    }
    output.push_str(value);
    Ok(())
}

pub(crate) fn flatten(
    value: &serde_json::Value,
    prefix: &str,
    output: &mut BTreeMap<String, String>,
) {
    if let Some(fields) = value.as_object() {
        for (name, value) in fields {
            let key = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            flatten(value, &key, output);
        }
    } else {
        output.insert(
            prefix.into(),
            value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned),
        );
    }
}
