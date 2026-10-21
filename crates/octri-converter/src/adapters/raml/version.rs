//! Version-specific syntax normalization, retaining immutable physical origins.
use crate::{
    ConversionError, Result,
    source::{Sources, child},
};
use serde_json::{Map, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Version {
    Raml08,
    Raml10,
}

impl Version {
    pub fn read(sources: &Sources) -> Result<Self> {
        match sources
            .text(&sources.root)?
            .trim_start_matches('\u{feff}')
            .lines()
            .next()
            .map(str::trim)
        {
            Some("#%RAML 0.8") => Ok(Self::Raml08),
            Some("#%RAML 1.0") => Ok(Self::Raml10),
            Some("#%RAML 1.0 Overlay" | "#%RAML 1.0 Extension") => Err(ConversionError::new(
                "RAML_COMPOSITION_UNSUPPORTED",
                "RAML overlays and extensions are not supported. Supply a fully composed RAML 1.0 API document; no partial conversion was produced.",
            )),
            _ => Err(ConversionError::new(
                "RAML_VERSION",
                "Entry point must be a RAML 0.8 or 1.0 API document",
            )),
        }
    }

    pub fn normalize(self, root: &mut Value, sources: &mut Sources) -> Result<()> {
        if let Some(version) = root.get("version") {
            if version.is_number() {
                root["version"] = Value::String(version.to_string());
            } else if !version.is_string() {
                return Err(ConversionError::new(
                    "RAML_API_VERSION",
                    "API version must be a string or numeric scalar",
                ));
            }
        }
        if self == Self::Raml10 {
            return normalize_type_group(root, "", sources);
        }
        for key in ["types", "uses", "annotationTypes"] {
            if root.get(key).is_some() {
                return Err(ConversionError::new(
                    "RAML_VERSION_FEATURE",
                    format!("{key} is not a RAML 0.8 declaration"),
                ));
            }
        }
        let mut origin_updates = Vec::new();
        for key in ["traits", "resourceTypes", "securitySchemes", "schemas"] {
            let Some(value) = root.get_mut(key) else {
                continue;
            };
            let groups = value.as_array().ok_or_else(|| {
                ConversionError::new(
                    "RAML_DECLARATIONS",
                    format!("RAML 0.8 {key} must be an array of declaration maps"),
                )
            })?;
            let mut declarations = Map::new();
            for (index, group) in groups.iter().enumerate() {
                let group = group
                    .as_object()
                    .filter(|map| !map.is_empty())
                    .ok_or_else(|| {
                        ConversionError::new(
                            "RAML_DECLARATIONS",
                            "Declaration array entries must be nonempty maps",
                        )
                    })?;
                for (name, declaration) in group {
                    if name.is_empty()
                        || declarations
                            .insert(name.clone(), declaration.clone())
                            .is_some()
                    {
                        return Err(ConversionError::new(
                            "RAML_DECLARATION_DUPLICATE",
                            format!("Duplicate or empty {key} declaration {name}"),
                        ));
                    }
                    let original = child(&child(&child("", key), &index.to_string()), name);
                    let normalized = child(&child("", key), name);
                    origin_updates.extend(declaration_origins(sources, &original, &normalized));
                }
            }
            *value = Value::Object(declarations);
        }
        // Logical names may overlap physical array pointers. Read every
        // original origin before installing any normalized aliases.
        sources.origins.extend(origin_updates);
        Ok(())
    }
}

/// Normalize only the declaration group, never names inside schema or example data.
pub(super) fn normalize_type_group(
    root: &mut Value,
    pointer: &str,
    sources: &mut Sources,
) -> Result<()> {
    if root.get("types").is_some() && root.get("schemas").is_some() {
        return Err(ConversionError::new(
            "RAML_TYPE_ALIAS",
            format!("types and schemas are mutually exclusive at {pointer}"),
        ));
    }
    let Some(map) = root.as_object_mut() else {
        return Ok(());
    };
    if let Some(declarations) = map.remove("schemas") {
        let original = child(pointer, "schemas");
        let normalized = child(pointer, "types");
        let origins = declaration_origins(sources, &original, &normalized);
        sources.origins.extend(origins);
        map.insert("types".into(), declarations);
    }
    if map.get("types").is_some_and(|value| !value.is_object()) {
        return Err(ConversionError::new(
            "RAML_DECLARATIONS",
            format!("RAML 1.0 types/schemas must be a declaration map at {pointer}"),
        ));
    }
    Ok(())
}

fn declaration_origins(
    sources: &Sources,
    original: &str,
    normalized: &str,
) -> Vec<(String, octri_parser::Address)> {
    let prefix = format!("{original}/");
    let mut origins = Vec::new();
    if let Some(address) = sources.origins.get(original) {
        origins.push((normalized.to_owned(), address.clone()));
    }
    // Only visit this declaration's descendants. A full origin-map scan per
    // declaration would turn a large, otherwise bounded input into quadratic work.
    for (pointer, address) in sources
        .origins
        .range(prefix.clone()..)
        .take_while(|(pointer, _)| pointer.starts_with(&prefix))
    {
        origins.push((
            format!("{normalized}{}", &pointer[original.len()..]),
            address.clone(),
        ));
    }
    origins
}
