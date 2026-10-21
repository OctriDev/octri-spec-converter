//! Trait/resource-type application with bounded expansion and literal parameters.
use super::template_value::{Kind, LocatedValue, merge};
use crate::source::Sources;
use crate::{ConversionError, Limits, Result};
use heck::{
    ToKebabCase, ToLowerCamelCase, ToShoutyKebabCase, ToShoutySnakeCase, ToSnakeCase,
    ToUpperCamelCase,
};
use inflector::string::{pluralize::to_plural, singularize::to_singular};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Templates<'a> {
    traits: LocatedValue,
    resource_types: LocatedValue,
    pub limits: &'a Limits,
    active: BTreeSet<String>,
    nodes: usize,
}

impl<'a> Templates<'a> {
    pub fn new(
        traits: &Value,
        resource_types: &Value,
        sources: &Sources,
        limits: &'a Limits,
    ) -> Self {
        Self {
            traits: LocatedValue::read(traits, "/traits", sources),
            resource_types: LocatedValue::read(resource_types, "/resourceTypes", sources),
            limits,
            active: BTreeSet::new(),
            nodes: 0,
        }
    }

    pub fn resource(&mut self, value: LocatedValue, path: &str) -> Result<LocatedValue> {
        let mut value = value;
        if let Kind::Object(fields) = &mut value.kind {
            for (name, value) in fields {
                if super::super::common::method(name)
                    && matches!(value.kind, Kind::Scalar(Value::Null))
                {
                    *value = LocatedValue::object(&value.origin);
                }
            }
        }
        if let Some(application) = value.get("type") {
            let template = self.apply(application, false, path, "")?;
            merge(template, value, self.limits, 0)
        } else {
            Ok(value)
        }
    }

    pub fn operation(
        &mut self,
        value: &LocatedValue,
        resource: &LocatedValue,
        path: &str,
        method: &str,
    ) -> Result<LocatedValue> {
        let mut result = LocatedValue::object(&value.origin);
        for applications in [resource.get("is"), value.get("is")].into_iter().flatten() {
            let Kind::Array(applications) = &applications.kind else {
                return Err(ConversionError::new(
                    "RAML_TRAITS",
                    "Trait applications must be an array",
                ));
            };
            for application in applications {
                result = merge(
                    result,
                    self.apply(application, true, path, method)?,
                    self.limits,
                    0,
                )?;
            }
        }
        let value = if matches!(value.kind, Kind::Scalar(Value::Null)) {
            LocatedValue::object(&value.origin)
        } else {
            value.clone()
        };
        merge(result, value, self.limits, 0)
    }

    fn apply(
        &mut self,
        application: &LocatedValue,
        is_trait: bool,
        path: &str,
        method: &str,
    ) -> Result<LocatedValue> {
        let (name, parameters) = if let Kind::Scalar(Value::String(name)) = &application.kind {
            (name.as_str(), None)
        } else {
            let Kind::Object(map) = &application.kind else {
                return Err(ConversionError::new(
                    "RAML_TEMPLATE",
                    "Template application needs one name",
                ));
            };
            if map.len() != 1 {
                return Err(ConversionError::new(
                    "RAML_TEMPLATE",
                    "Template application needs one name",
                ));
            }
            let (name, parameters) = map
                .iter()
                .next()
                .ok_or_else(|| ConversionError::new("RAML_TEMPLATE", "Template name missing"))?;
            (name.as_str(), Some(parameters))
        };
        let key = format!("{}:{name}", if is_trait { "trait" } else { "resourceType" });
        if !self.active.insert(key.clone()) || self.active.len() > self.limits.max_depth {
            return Err(ConversionError::new(
                "RAML_TEMPLATE_CYCLE",
                "Template application is cyclic or too deep",
            ));
        }
        let template = if is_trait {
            &self.traits
        } else {
            &self.resource_types
        }
        .get(name)
        .ok_or_else(|| ConversionError::new("RAML_TEMPLATE", format!("Unknown template {name}")))?
        .clone();
        let resource_path = path.replace("{ext}", "");
        let mut variables = BTreeMap::from([
            (
                "resourcePath".into(),
                LocatedValue::scalar(serde_json::json!(resource_path), &application.origin),
            ),
            (
                "resourcePathName".into(),
                LocatedValue::scalar(
                    serde_json::json!(
                        resource_path
                            .rsplit('/')
                            .find(|part| !part.is_empty() && !part.contains(['{', '}']))
                            .unwrap_or("")
                    ),
                    &application.origin,
                ),
            ),
        ]);
        if is_trait {
            variables.insert(
                "methodName".into(),
                LocatedValue::scalar(serde_json::json!(method), &application.origin),
            );
        }
        if let Some(parameters) = parameters {
            let Kind::Object(map) = &parameters.kind else {
                return Err(ConversionError::new(
                    "RAML_TEMPLATE",
                    "Template parameters must be an object",
                ));
            };
            if map.keys().any(|key| {
                matches!(
                    key.as_str(),
                    "resourcePath" | "resourcePathName" | "methodName"
                )
            }) {
                return Err(ConversionError::new(
                    "RAML_RESERVED_PARAMETER",
                    "Template parameters cannot replace reserved RAML parameters",
                ));
            }
            variables.extend(map.iter().map(|(key, value)| (key.clone(), value.clone())));
        }
        let mut result = self.substitute(&template, &variables, 0)?;
        if !is_trait && result.get("type").is_some() {
            result = self.resource(result, path)?;
        }
        if is_trait && result.get("is").is_some() {
            result = self.operation(
                &result,
                &LocatedValue::object(&application.origin),
                path,
                method,
            )?;
        }
        self.active.remove(&key);
        Ok(result)
    }

    fn substitute(
        &mut self,
        value: &LocatedValue,
        variables: &BTreeMap<String, LocatedValue>,
        depth: usize,
    ) -> Result<LocatedValue> {
        self.nodes += 1;
        if self.nodes > self.limits.max_nodes || depth > self.limits.max_depth {
            return Err(ConversionError::new(
                "RAML_TEMPLATE_LIMIT",
                "Expanded templates exceed configured node or depth limits",
            ));
        }
        let kind = match &value.kind {
            Kind::Scalar(Value::String(text)) => {
                if text.starts_with("<<") && text.ends_with(">>") && text.matches("<<").count() == 1
                {
                    let name = text[2..text.len() - 2].trim();
                    if let Some(value) = variables.get(name) {
                        self.charge_parameter(value, depth)?;
                        return Ok(value.clone());
                    }
                }
                Kind::Scalar(Value::String(substitute_string(
                    text,
                    variables,
                    self.limits.max_document_bytes,
                )?))
            }
            Kind::Object(map) => {
                let mut result = BTreeMap::new();
                for (key, value) in map {
                    let key = substitute_string(key, variables, self.limits.max_document_bytes)?;
                    let value = self.substitute(value, variables, depth + 1)?;
                    if result.insert(key, value).is_some() {
                        return Err(ConversionError::new(
                            "RAML_TEMPLATE_KEY",
                            "Substitution produces duplicate keys",
                        ));
                    }
                }
                Kind::Object(result)
            }
            Kind::Array(values) => Kind::Array(
                values
                    .iter()
                    .map(|value| self.substitute(value, variables, depth + 1))
                    .collect::<Result<Vec<_>>>()?,
            ),
            scalar => scalar.clone(),
        };
        Ok(LocatedValue {
            origin: value.origin.clone(),
            kind,
        })
    }

    fn charge_parameter(&mut self, value: &LocatedValue, depth: usize) -> Result<()> {
        let mut pending = vec![(value, depth)];
        while let Some((value, depth)) = pending.pop() {
            self.nodes = self.nodes.saturating_add(1);
            if self.nodes > self.limits.max_nodes || depth > self.limits.max_depth {
                return Err(ConversionError::new(
                    "RAML_TEMPLATE_LIMIT",
                    "Expanded template parameters exceed configured node or depth limits",
                ));
            }
            match &value.kind {
                Kind::Object(fields) => {
                    pending.extend(fields.values().map(|value| (value, depth + 1)))
                }
                Kind::Array(values) => {
                    pending.extend(values.iter().map(|value| (value, depth + 1)))
                }
                Kind::Scalar(_) => {}
            }
        }
        Ok(())
    }
}

fn substitute_string(
    text: &str,
    variables: &BTreeMap<String, LocatedValue>,
    limit: usize,
) -> Result<String> {
    let mut result = String::new();
    let mut tail = text;
    while let Some((before, rest)) = tail.split_once("<<") {
        let (name, after) = rest.split_once(">>").ok_or_else(|| {
            ConversionError::new("RAML_TEMPLATE_PARAMETER", "Unclosed template parameter")
        })?;
        let mut expression = name.split('|').map(str::trim);
        let name = expression.next().unwrap_or("");
        let value = variables.get(name).ok_or_else(|| {
            ConversionError::new(
                "RAML_TEMPLATE_PARAMETER",
                format!("Supply template parameter {name}"),
            )
        })?;
        let value = value.to_value();
        let mut value = value
            .as_str()
            .map_or_else(|| value.to_string(), str::to_owned);
        for transform in expression {
            value = match transform {
                "!singularize" => to_singular(&value),
                "!pluralize" => to_plural(&value),
                "!uppercase" => value.to_uppercase(),
                "!lowercase" => value.to_lowercase(),
                "!lowercamelcase" => value.to_lower_camel_case(),
                "!uppercamelcase" => value.to_upper_camel_case(),
                "!lowerunderscorecase" => value.to_snake_case(),
                "!upperunderscorecase" => value.to_shouty_snake_case(),
                "!lowerhyphencase" => value.to_kebab_case(),
                "!upperhyphencase" => value.to_shouty_kebab_case(),
                _ => {
                    return Err(ConversionError::new(
                        "RAML_TEMPLATE_TRANSFORM",
                        format!("Unknown RAML parameter transform {transform}"),
                    ));
                }
            };
            if value.len() > limit {
                return Err(ConversionError::new(
                    "RAML_TEMPLATE_BYTES",
                    "Transformed template exceeds byte limit",
                ));
            }
        }
        if result
            .len()
            .saturating_add(before.len())
            .saturating_add(value.len())
            .saturating_add(after.len())
            > limit
        {
            return Err(ConversionError::new(
                "RAML_TEMPLATE_BYTES",
                "Expanded template value exceeds byte limit",
            ));
        }
        result.push_str(before);
        result.push_str(&value);
        tail = after;
    }
    result.push_str(tail);
    Ok(result)
}
