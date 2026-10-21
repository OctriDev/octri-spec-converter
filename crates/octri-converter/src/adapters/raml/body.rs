//! RAML 0.8 body declarations are schemas or form parameters, not RAML types.
use super::Context;
use crate::adapters::references::parse_schema_text;
use crate::{Classification, ConversionError, Result, source::child};
use octri_parser::{Address, SchemaDialect};
use serde_json::{Map, Value, json};

impl Context<'_> {
    pub(super) fn legacy_body_schema(
        &mut self,
        declaration: &Value,
        pointer: &str,
        xml: bool,
    ) -> Result<Value> {
        let Some(map) = declaration.as_object() else {
            if declaration.is_null() {
                return Ok(json!({}));
            }
            return Err(ConversionError::new(
                "RAML_BODY",
                "RAML 0.8 body declaration must be a map",
            ));
        };
        if map.contains_key("schema") && map.contains_key("formParameters") {
            return Err(ConversionError::new(
                "RAML_BODY",
                "Body cannot combine schema and formParameters",
            ));
        }
        for (name, value) in map {
            if !matches!(name.as_str(), "schema" | "formParameters" | "example") {
                self.record(
                    &child(pointer, name),
                    &[],
                    "RAML_BODY_FACET",
                    Classification::Omitted,
                    format!("{name} is not a RAML 0.8 body field; retained in original source"),
                    Some(value),
                );
            }
        }
        if let Some(parameters) = map.get("formParameters") {
            let parameters = parameters
                .as_object()
                .ok_or_else(|| ConversionError::new("RAML_FORM", "formParameters must be a map"))?;
            let mut properties = Map::new();
            let mut required = Vec::new();
            for (name, declaration) in parameters {
                let at = child(&child(pointer, "formParameters"), name);
                properties.insert(name.clone(), self.form_parameter_schema(declaration, &at)?);
                if self.parameter_required(declaration, false)? {
                    required.push(name.clone());
                }
            }
            let mut schema = json!({"type":"object","properties":properties});
            if !required.is_empty() {
                schema["required"] = json!(required);
            }
            return Ok(schema);
        }
        if let Some(schema) = map.get("schema") {
            return self.legacy_schema(schema, &child(pointer, "schema"), xml);
        }
        Ok(json!({}))
    }

    fn legacy_schema(&mut self, schema: &Value, pointer: &str, xml: bool) -> Result<Value> {
        let (schema, pointer) = if let Some(name) = schema
            .as_str()
            .filter(|name| self.legacy_schemas.contains_key(*name))
        {
            (self.legacy_schemas[name].clone(), child("/schemas", name))
        } else {
            (schema.clone(), pointer.to_owned())
        };
        let address = self
            .sources
            .origins
            .get(&pointer)
            .cloned()
            .unwrap_or_else(|| Address {
                uri: self.sources.root.clone(),
                pointer: pointer.clone(),
            });
        if super::schema_fragments::kind(&address, self.sources)
            .is_some_and(|kind| matches!(kind, super::schema_use::SchemaKind::Xml) && !xml)
        {
            return Err(ConversionError::new(
                "RAML_SCHEMA_PLACEMENT",
                "XML Schema body requires an XML-compatible media type",
            ));
        }
        if let Some(schema) = super::schema_fragments::import(
            &address,
            &schema,
            self.sources,
            &mut self.schemas,
            &mut self.xml_schemas,
            self.trace,
            SchemaDialect::Draft4,
        )? {
            return Ok(schema);
        }
        let parsed = if let Some(text) = schema.as_str() {
            if super::schema_use::document_text(text).starts_with('<') {
                if !xml {
                    return Err(ConversionError::new(
                        "RAML_SCHEMA_PLACEMENT",
                        "XML Schema body requires an XML-compatible media type",
                    ));
                }
                return self
                    .xml_schemas
                    .import(text, address, self.sources, self.trace);
            }
            parse_schema_text(text, &address, self.sources)?
        } else {
            schema.clone()
        };
        if !parsed.is_object() && !parsed.is_boolean() {
            return Err(ConversionError::new(
                "RAML_SCHEMA",
                "Body schema must name a declared schema or contain a JSON Schema document",
            ));
        }
        if parsed
            .get("$schema")
            .and_then(Value::as_str)
            .is_some_and(|uri| SchemaDialect::from_uri(uri).is_none())
        {
            self.record(
                &pointer,
                &[],
                "RAML_SCHEMA_DIALECT",
                Classification::Blocked,
                "Declared schema dialect requires a matching parser implementation or authored replacement; it cannot be treated as RAML types or modern JSON Schema",
                Some(&parsed),
            );
            return Ok(json!({}));
        }
        self.record(
            &pointer,
            &[],
            "RAML_BODY_SCHEMA",
            Classification::Transformed,
            "RAML body schema imported through the shared parser with its own resource and reference scope",
            Some(&schema),
        );
        if schema.is_string() {
            self.schemas
                .import_serialized(parsed, address, self.sources, SchemaDialect::Draft4)
        } else {
            self.schemas
                .import_projected(parsed, address, self.sources, SchemaDialect::Draft4)
        }
    }
}
