//! RAML type expressions and facets. Examples remain literal instance data.
use super::super::references::{Schemas, parse_schema_text};
use super::schema_use::{self, SchemaUse};
use crate::{Classification, ConversionError, Result};
use crate::{
    source::{Sources, child},
    trace::Trace,
};
use octri_parser::Address;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Types<'a> {
    pub names: &'a BTreeSet<String>,
    pub declarations: &'a Map<String, Value>,
    pub external_names: &'a BTreeMap<String, schema_use::SchemaKind>,
    pub sources: &'a mut Sources,
    pub trace: &'a mut Trace,
    pub schemas: &'a mut Schemas,
    pub xml_schemas: &'a mut super::xml_schemas::XmlSchemas,
}

impl Types<'_> {
    pub fn schema(
        &mut self,
        value: &Value,
        pointer: &str,
        depth: usize,
        usage: SchemaUse,
    ) -> Result<Value> {
        if depth > self.sources.limits.max_depth {
            return Err(ConversionError::new(
                "RAML_TYPE_DEPTH",
                "Type nesting exceeds configured depth",
            ));
        }
        let external = schema_use::kind(value, pointer, self.sources, self.external_names);
        if external.is_some_and(|kind| !usage.permits(kind)) {
            return Err(ConversionError::new(
                "RAML_SCHEMA_PLACEMENT",
                format!(
                    "JSON/XML schema documents cannot be used as RAML parameters, properties or type expressions, or in incompatible body media at {pointer}"
                ),
            ));
        }
        if let Some(site) = self.sources.origins.get(pointer).cloned()
            && let Some(schema) = super::schema_fragments::import(
                &site,
                value,
                self.sources,
                self.schemas,
                self.xml_schemas,
                self.trace,
                octri_parser::SchemaDialect::default(),
            )?
        {
            return Ok(schema);
        }
        if self
            .sources
            .origins
            .get(pointer)
            .is_some_and(|origin| origin.uri != self.sources.root && origin.uri.ends_with(".json"))
            || value.get("$schema").is_some()
        {
            let address = self
                .sources
                .origins
                .get(pointer)
                .cloned()
                .unwrap_or_else(|| Address {
                    uri: self.sources.root.clone(),
                    pointer: pointer.into(),
                });
            return self.schemas.import_projected(
                value.clone(),
                address,
                self.sources,
                octri_parser::SchemaDialect::default(),
            );
        }
        if value.is_null() {
            return Ok(json!({"type":"string"}));
        }
        if let Some(expression) = value.as_str() {
            return self.expression(expression, pointer, depth + 1, usage);
        }
        let map = value.as_object().ok_or_else(|| {
            ConversionError::new(
                "RAML_TYPE",
                "Type declaration must be an expression or object",
            )
        })?;
        if map.contains_key("type") && map.contains_key("schema") {
            return Err(ConversionError::new(
                "RAML_TYPE_ALIAS",
                format!("type and schema are mutually exclusive at {pointer}"),
            ));
        }
        let base_key = if map.contains_key("schema") {
            "schema"
        } else {
            "type"
        };
        if external.is_some()
            && map.keys().any(|key| {
                let metadata = matches!(
                    key.as_str(),
                    "type" | "schema" | "description" | "displayName" | "example" | "examples"
                );
                let annotation = key.starts_with('(') && key.ends_with(')');
                !(metadata || annotation)
            })
        {
            return Err(ConversionError::new(
                "RAML_SCHEMA_SPECIALIZATION",
                format!(
                    "Schema document wrappers may add only documentation, examples and annotations at {pointer}"
                ),
            ));
        }
        let mut schema = match map.get(base_key) {
            Some(Value::Array(bases)) => {
                json!({"allOf":bases.iter().map(|base|self.schema(base,&child(pointer,base_key),depth+1,SchemaUse::RamlType)).collect::<Result<Vec<_>>>()?})
            }
            Some(base) => self.schema(base, &child(pointer, base_key), depth + 1, usage)?,
            None => {
                json!({"type":if map.contains_key("properties"){"object"}else if map.contains_key("items"){"array"}else{"string"}})
            }
        };
        let original = value.clone();
        for key in [
            "description",
            "default",
            "enum",
            "minimum",
            "maximum",
            "multipleOf",
            "minLength",
            "maxLength",
            "pattern",
            "minItems",
            "maxItems",
            "uniqueItems",
            "minProperties",
            "maxProperties",
            "additionalProperties",
        ] {
            if let Some(value) = map.get(key) {
                schema[key] = value.clone();
            }
        }
        if let Some(display) = map.get("displayName") {
            schema["title"] = display.clone();
        }
        if let Some(format) = map.get("format")
            && super::formats::apply(&mut schema, format)?
        {
            self.record(&child(pointer,"format"), "RAML_HTTP_DATE", Classification::Inferred, "HTTP date lexical forms preserved. Review calendar and weekday validation in the receiving HTTP runtime; date-time would describe a different wire format.", Some(format));
        }
        if let Some(items) = map.get("items") {
            schema["items"] = self.schema(
                items,
                &child(pointer, "items"),
                depth + 1,
                SchemaUse::RamlType,
            )?;
        }
        if let Some(fields) = map.get("properties").and_then(Value::as_object) {
            let mut properties = Map::new();
            let mut patterns = Map::new();
            let mut required = Vec::new();
            for (name, declaration) in fields {
                let optional = name.ends_with('?');
                let name = name.strip_suffix('?').unwrap_or(name);
                let source_name = if optional {
                    format!("{name}?")
                } else {
                    name.into()
                };
                let at = child(&child(pointer, "properties"), &source_name);
                let field = self.schema(declaration, &at, depth + 1, SchemaUse::RamlType)?;
                if name.starts_with('/') && name.ends_with('/') && name.len() > 1 {
                    patterns.insert(name[1..name.len() - 1].into(), field);
                } else {
                    if declaration
                        .get("required")
                        .and_then(Value::as_bool)
                        .unwrap_or(!optional)
                    {
                        required.push(name.to_owned());
                    }
                    if properties.insert(name.into(), field).is_some() {
                        return Err(ConversionError::new(
                            "RAML_PROPERTY",
                            "Optional and required declarations use the same property name",
                        ));
                    }
                }
            }
            schema["properties"] = Value::Object(properties);
            if !patterns.is_empty() {
                schema["patternProperties"] = Value::Object(patterns);
            }
            if !required.is_empty() {
                schema["required"] = json!(required);
            }
        }
        if let Some(example) = map.get("example") {
            let literal = if example.get("value").is_some() {
                example["value"].clone()
            } else {
                example.clone()
            };
            schema["example"] = literal;
        }
        if let Some(examples) = map.get("examples").and_then(Value::as_object) {
            schema["examples"] = json!(
                examples
                    .values()
                    .map(|example| example.get("value").unwrap_or(example))
                    .collect::<Vec<_>>()
            );
        }
        if let Some(discriminator) = map.get("discriminator").and_then(Value::as_str) {
            schema["discriminator"] = json!({"propertyName":discriminator});
        }
        if let Some(value) = map.get("discriminatorValue") {
            schema["x-raml-discriminator-value"] = value.clone();
            self.record(pointer,"RAML_DISCRIMINATOR_VALUE",Classification::Inferred,"Discriminator value retained as metadata. Review inherited discriminator property and authored const mapping.",Some(value));
        }
        super::serialization::apply(
            &mut schema,
            value,
            pointer,
            self.declarations,
            self.sources,
            self.trace,
        )?;
        let known = [
            "type",
            "schema",
            "description",
            "displayName",
            "default",
            "enum",
            "minimum",
            "maximum",
            "multipleOf",
            "minLength",
            "maxLength",
            "pattern",
            "minItems",
            "maxItems",
            "uniqueItems",
            "minProperties",
            "maxProperties",
            "additionalProperties",
            "format",
            "items",
            "properties",
            "required",
            "example",
            "examples",
            "discriminator",
            "discriminatorValue",
            "xml",
            "fileTypes",
        ];
        for (key, value) in map {
            if key.starts_with('(') && key.ends_with(')') {
                schema[&format!("x-raml-annotation-{}", key.trim_matches(['(', ')']))] =
                    value.clone();
            } else if !known.contains(&key.as_str()) {
                self.record(&child(pointer,key),"RAML_FACET",Classification::Omitted,format!("Facet {key} requires an authored schema mapping; original declaration retained"),Some(value));
            }
        }
        self.record(
            pointer,
            "RAML_TYPE",
            Classification::Transformed,
            "RAML type constraints mapped independently from examples",
            Some(&original),
        );
        Ok(schema)
    }

    fn expression(
        &mut self,
        raw: &str,
        pointer: &str,
        depth: usize,
        usage: SchemaUse,
    ) -> Result<Value> {
        if depth > self.sources.limits.max_depth {
            return Err(ConversionError::new(
                "RAML_TYPE_DEPTH",
                "Type expression exceeds nesting limit",
            ));
        }
        let expression = schema_use::document_text(raw);
        if schema_use::kind(
            &Value::String(expression.into()),
            pointer,
            self.sources,
            self.external_names,
        )
        .is_some_and(|kind| !usage.permits(kind))
        {
            return Err(ConversionError::new(
                "RAML_SCHEMA_PLACEMENT",
                format!(
                    "Schema document cannot participate in a RAML type expression at {pointer}"
                ),
            ));
        }
        if expression.starts_with('<') {
            let physical = self
                .sources
                .origins
                .get(pointer)
                .cloned()
                .unwrap_or_else(|| Address {
                    uri: self.sources.root.clone(),
                    pointer: pointer.into(),
                });
            return self
                .xml_schemas
                .import(raw, physical, self.sources, self.trace);
        }
        if expression.starts_with('{') {
            let address = self
                .sources
                .origins
                .get(pointer)
                .cloned()
                .unwrap_or_else(|| Address {
                    uri: self.sources.root.clone(),
                    pointer: pointer.into(),
                });
            let schema = parse_schema_text(expression, &address, self.sources)?;
            return self.schemas.import_serialized(
                schema,
                address,
                self.sources,
                octri_parser::SchemaDialect::default(),
            );
        }
        let alternatives = split_union(expression)?;
        if alternatives.len() > 1 {
            return Ok(
                json!({"anyOf":alternatives.into_iter().map(|part|self.expression(part,pointer,depth+1,SchemaUse::RamlType)).collect::<Result<Vec<_>>>()?}),
            );
        }
        if let Some(inner) = expression.strip_suffix("[]") {
            return Ok(
                json!({"type":"array","items":self.expression(inner,pointer,depth+1,SchemaUse::RamlType)?}),
            );
        }
        if expression.starts_with('(') && expression.ends_with(')') {
            return self.expression(
                &expression[1..expression.len() - 1],
                pointer,
                depth + 1,
                SchemaUse::RamlType,
            );
        }
        Ok(match expression {
            "any" => json!({}),
            "nil" => json!({"type":"null"}),
            "string" | "boolean" | "number" | "integer" | "object" => json!({"type":expression}),
            "array" => json!({"type":"array","items":{}}),
            "file" => json!({"type":"string","format":"binary"}),
            "date-only" => json!({"type":"string","format":"date"}),
            "datetime" => json!({"type":"string","format":"date-time"}),
            "time-only" => {
                json!({"type":"string","pattern":"^[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\\.[0-9]+)?$","x-raml-type":"time-only"})
            }
            "datetime-only" => {
                json!({"type":"string","pattern":"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\\.[0-9]+)?$","x-raml-type":"datetime-only"})
            }
            name if self.names.contains(name) => {
                json!({"$ref":format!("#{}",child("/components/schemas",name))})
            }
            name => {
                return Err(ConversionError::new(
                    "RAML_TYPE",
                    format!("Unknown type {name} at {pointer}"),
                ));
            }
        })
    }

    fn record(
        &mut self,
        pointer: &str,
        code: &str,
        class: Classification,
        message: impl Into<String>,
        evidence: Option<&Value>,
    ) {
        self.trace.record(
            self.sources,
            &self.sources.root,
            pointer,
            &[],
            code,
            class,
            message,
            evidence,
        );
    }
}

pub(super) fn split_union(expression: &str) -> Result<Vec<&str>> {
    let mut depth = 0usize;
    let mut start = 0;
    let mut parts = Vec::new();
    for (index, ch) in expression.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    ConversionError::new("RAML_TYPE", "Unbalanced type expression")
                })?
            }
            '|' if depth == 0 => {
                parts.push(expression[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(ConversionError::new(
            "RAML_TYPE",
            "Unbalanced type expression",
        ));
    }
    parts.push(expression[start..].trim());
    if parts.iter().any(|part| part.is_empty()) {
        return Err(ConversionError::new("RAML_TYPE", "Empty type union member"));
    }
    Ok(parts)
}
