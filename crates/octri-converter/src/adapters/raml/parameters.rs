//! RAML 0.8 named parameters have optional defaults and a distinct repeat facet.
use super::{Context, version::Version};
use crate::{Classification, ConversionError, Result, source::child};
use serde_json::{Value, json};

impl Context<'_> {
    pub(super) fn parameter_name<'a>(&self, name: &'a str) -> (&'a str, bool) {
        if self.version == Version::Raml10 && name.ends_with('?') {
            (&name[..name.len() - 1], true)
        } else {
            (name, false)
        }
    }

    pub(super) fn parameter_required(&self, declaration: &Value, optional: bool) -> Result<bool> {
        if self.version == Version::Raml10 {
            return Ok(declaration
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(!optional));
        }
        if let Some(alternatives) = declaration.as_array() {
            if alternatives.is_empty() {
                return Err(ConversionError::new(
                    "RAML_PARAMETER",
                    "Parameter alternatives cannot be empty",
                ));
            }
            return alternatives.iter().try_fold(true, |required, alternative| {
                Ok(required & self.parameter_required(alternative, optional)?)
            });
        }
        legacy_boolean(declaration.get("required"))
    }

    pub(super) fn parameter_schema(&mut self, declaration: &Value, pointer: &str) -> Result<Value> {
        self.parameter_schema_at(declaration, pointer, 0, false)
    }

    pub(super) fn form_parameter_schema(
        &mut self,
        declaration: &Value,
        pointer: &str,
    ) -> Result<Value> {
        self.parameter_schema_at(declaration, pointer, 0, true)
    }

    fn parameter_schema_at(
        &mut self,
        declaration: &Value,
        pointer: &str,
        depth: usize,
        allow_file: bool,
    ) -> Result<Value> {
        if depth > self.sources.limits.max_depth {
            return Err(ConversionError::new(
                "RAML_PARAMETER_DEPTH",
                "Parameter alternatives exceed configured depth",
            ));
        }
        if self.version == Version::Raml10 {
            return self.schema_at(declaration, pointer, super::schema_use::SchemaUse::RamlType);
        }
        if let Some(alternatives) = declaration.as_array() {
            if alternatives.is_empty() {
                return Err(ConversionError::new(
                    "RAML_PARAMETER",
                    "Parameter alternatives cannot be empty",
                ));
            }
            let schemas = alternatives
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    self.parameter_schema_at(
                        value,
                        &child(pointer, &index.to_string()),
                        depth + 1,
                        allow_file,
                    )
                })
                .collect::<Result<Vec<_>>>()?;
            return Ok(json!({"anyOf":schemas}));
        }
        if !declaration.is_null() && !declaration.is_object() {
            return Err(ConversionError::new(
                "RAML_PARAMETER",
                "RAML 0.8 parameter must be a map or array of maps",
            ));
        }
        self.parameter_required(declaration, false)?;
        let repeat = legacy_boolean(declaration.get("repeat"))?;
        let mut normalized = if declaration.is_null() {
            json!({})
        } else {
            declaration.clone()
        };
        let known = [
            "displayName",
            "description",
            "type",
            "enum",
            "pattern",
            "minLength",
            "maxLength",
            "minimum",
            "maximum",
            "example",
            "repeat",
            "required",
            "default",
        ];
        if let Some(map) = normalized.as_object_mut() {
            for key in map
                .keys()
                .filter(|key| !known.contains(&key.as_str()))
                .cloned()
                .collect::<Vec<_>>()
            {
                if let Some(value) = map.remove(&key) {
                    self.record(
                        &child(pointer, &key),
                        &[],
                        "RAML_PARAMETER_FACET",
                        Classification::Omitted,
                        format!("{key} is not a RAML 0.8 named-parameter facet; retained in original source without activating RAML 1.0 semantics"),
                        Some(&value),
                    );
                }
            }
        }
        if let Some(kind) = normalized.get("type") {
            let kind = kind.as_str().ok_or_else(|| {
                ConversionError::new(
                    "RAML_PARAMETER",
                    "RAML 0.8 parameter type must be a primitive type name",
                )
            })?;
            if kind == "file" && !allow_file {
                return Err(ConversionError::new(
                    "RAML_PARAMETER",
                    "RAML 0.8 file parameters are only supported in form bodies",
                ));
            }
            if !matches!(
                kind,
                "string" | "number" | "integer" | "boolean" | "date" | "file"
            ) {
                return Err(ConversionError::new(
                    "RAML_PARAMETER",
                    format!("Unsupported RAML 0.8 parameter type {kind}"),
                ));
            }
            if kind == "date" {
                normalized["type"] = json!("datetime");
                normalized["format"] = json!("rfc2616");
            }
        }
        if let Some(map) = normalized.as_object_mut() {
            map.remove("repeat");
        }
        let schema = self.schema(&normalized, pointer)?;
        Ok(if repeat {
            json!({"type":"array","items":schema})
        } else {
            schema
        })
    }
}

fn legacy_boolean(value: Option<&Value>) -> Result<bool> {
    match value {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        _ => Err(ConversionError::new(
            "RAML_PARAMETER",
            "required and repeat facets must be booleans",
        )),
    }
}
