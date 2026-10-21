//! Shared XSD projection, cached by physical origin and expanded schema content.
use crate::{
    ConversionError, Result,
    source::{Sources, hash},
    trace::Trace,
};
use octri_parser::Address;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct XmlSchemas {
    cache: BTreeMap<(Address, String), super::super::wsdl::ProjectedSchema>,
    pub definitions: Map<String, Value>,
}

impl XmlSchemas {
    pub fn import(
        &mut self,
        text: &str,
        physical: Address,
        sources: &mut Sources,
        trace: &mut Trace,
    ) -> Result<Value> {
        self.import_selected(text, physical, None, sources, trace)
    }

    pub fn import_selected(
        &mut self,
        text: &str,
        physical: Address,
        selector: Option<&str>,
        sources: &mut Sources,
        trace: &mut Trace,
    ) -> Result<Value> {
        let key = (physical.clone(), hash(text.as_bytes()));
        if !self.cache.contains_key(&key) {
            let identity = format!("xml:{}#{}:{}", physical.uri, physical.pointer, key.1);
            sources.admit_derived(&identity, text.len(), 0)?;
            let mut projected = super::super::wsdl::project_schema_text(
                text, &physical, sources, trace,
            )
            .map_err(|mut error| {
                if error.diagnostics.is_empty() {
                    error.diagnostics.push(octri_parser::Diagnostic {
                        code: error.code.clone(),
                        severity: octri_parser::Severity::Error,
                        message: error.message.clone(),
                        location: sources.physical_location(&physical),
                        related: Vec::new(),
                    });
                }
                error
            })?;
            for (name, value) in std::mem::take(&mut projected.definitions) {
                if let Some(previous) = self.definitions.insert(name, value.clone())
                    && previous != value
                {
                    return Err(ConversionError::new(
                        "XSD_COMPONENT_COLLISION",
                        "Distinct XML declarations produced the same component identity",
                    ));
                }
            }
            self.cache.insert(key.clone(), projected);
        }
        let projected = self.cache.get(&key).ok_or_else(|| {
            ConversionError::new("XSD_CACHE", "Projected XML Schema cache is missing")
        })?;
        let Some(name) = selector else {
            return projected.root.clone().ok_or_else(|| {
                ConversionError::new(
                    "XSD_BODY_ROOT",
                    "XML Schema body requires a global element declaration",
                )
            });
        };
        match (
            projected.elements.get(name),
            projected.complex_types.get(name),
        ) {
            (Some(_), Some(_)) => Err(ConversionError::new(
                "RAML_SCHEMA_FRAGMENT",
                "XML fragment is ambiguous between a global element and complex type",
            )),
            (Some(schema), None) => Ok(schema.clone()),
            (None, Some(schema)) => {
                trace.record(sources, &physical.uri, &physical.pointer, &[],
                    "RAML_XSD_TYPE_SERIALIZATION", crate::Classification::Inferred,
                    "Selected XML complex type has no top-level element name. Its value model can be validated, but this type must not be used alone to serialize an XML instance.", None);
                Ok(schema.clone())
            }
            (None, None) => Err(ConversionError::new(
                "RAML_SCHEMA_FRAGMENT",
                "XML fragment does not select a global element or complex type in the included schema namespace",
            )),
        }
    }
}
