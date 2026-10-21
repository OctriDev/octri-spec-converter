//! Foreign API object references and parser-backed JSON Schema imports.
use crate::{Classification, ConversionError, Result};
use crate::{
    source::{Sources, child, hash},
    trace::Trace,
};
use octri_parser::{Address, SchemaDialect, SchemaRoot};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub(crate) struct Located {
    pub value: Value,
    pub address: Address,
}

/// Decoded schema text has no independent source offsets. Findings belong to
/// the original scalar, while references retain that scalar's physical base.
pub(crate) fn parse_schema_text(
    text: &str,
    physical: &Address,
    sources: &Sources,
) -> Result<Value> {
    let schema = octri_parser::parse_source(&physical.uri, text.into(), &sources.limits.parser())
        .map_err(|mut error| {
            let location = sources.physical_location(physical);
            error.message = format!("Invalid embedded schema text: {}", error.message);
            for diagnostic in &mut error.diagnostics {
                diagnostic.location = location.clone();
                diagnostic.message = format!("Embedded schema text: {}", diagnostic.message);
                for related in &mut diagnostic.related {
                    *related = location.clone();
                }
            }
            error
        })?
        .document;
    Ok(schema)
}

/// Reference Objects use document pointers. JSON Schema resource IDs, anchors
/// and dynamic references are handled by the parser through `Schemas` below.
pub(crate) fn resolve(
    sources: &mut Sources,
    mut value: Located,
    trace: &mut Trace,
) -> Result<Located> {
    let mut seen = BTreeSet::new();
    while let Some(reference) = value.value.get("$ref").and_then(Value::as_str) {
        if !seen.insert(value.address.clone()) || seen.len() > sources.limits.max_depth {
            return Err(ConversionError::new(
                "REFERENCE_CYCLE",
                "API object reference cycle has no concrete object",
            ));
        }
        if value.value.as_object().is_some_and(|map| map.len() > 1) {
            trace.record(sources,&value.address.uri,&value.address.pointer,&[],"FOREIGN_REFERENCE_SIBLINGS",Classification::Omitted,"Reference Object siblings are not activated during conversion; review any source-specific sibling behavior",Some(&value.value));
        }
        let target = address(&value.address.uri, reference)?;
        let document = sources.parse(&target.uri, false)?;
        let resolved = document.pointer(&target.pointer).cloned().ok_or_else(|| {
            ConversionError::new(
                "UNRESOLVED_REFERENCE",
                format!("Missing source {}#{}", target.uri, target.pointer),
            )
        })?;
        value = Located {
            value: resolved,
            address: target,
        };
    }
    Ok(value)
}

pub(crate) fn address(base: &str, reference: &str) -> Result<Address> {
    let mut uri = url::Url::parse(base)
        .and_then(|base| base.join(reference))
        .map_err(|error| ConversionError::new("REFERENCE_URI", error.to_string()))?;
    let pointer = percent_encoding::percent_decode_str(uri.fragment().unwrap_or(""))
        .decode_utf8()
        .map_err(|_| {
            ConversionError::new(
                "REFERENCE_FRAGMENT",
                "Reference fragment must be valid UTF-8",
            )
        })?
        .into_owned();
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(ConversionError::new(
            "REFERENCE_FRAGMENT",
            "API object references require JSON Pointers; schema anchors are resolved by the schema importer",
        ));
    }
    uri.set_fragment(None);
    Ok(Address {
        uri: uri.into(),
        pointer,
    })
}

/// Collect physical roots before bundling. Complete source snapshots preserve
/// local fragments, enclosing resources, cycles and dynamic scope.
#[derive(Debug, Default)]
pub(crate) struct Schemas {
    imported: BTreeMap<Address, ImportedSchema>,
    derived: BTreeMap<String, (Value, Address)>,
    contexts: BTreeMap<Address, SchemaDialect>,
}

#[derive(Debug)]
struct ImportedSchema {
    name: String,
    dialect: SchemaDialect,
}

impl Schemas {
    /// Preserve the complete enclosing resource, including sibling definitions
    /// needed by identifiers, anchors and dynamic reference scope.
    pub fn register_context(&mut self, address: Address, dialect: SchemaDialect) -> Result<()> {
        if let Some(previous) = self.contexts.insert(address, dialect)
            && previous != dialect
        {
            return Err(ConversionError::new(
                "SCHEMA_DIALECT_CONFLICT",
                "The same schema context has conflicting default dialects",
            ));
        }
        Ok(())
    }

    /// RAML templates can transform a schema while retaining its physical base.
    /// An unchanged value imports that source root directly. A transformed value
    /// needs a derived resource so the parser never rereads an older template.
    pub fn import_projected(
        &mut self,
        schema: Value,
        physical: Address,
        sources: &mut Sources,
        dialect: SchemaDialect,
    ) -> Result<Value> {
        let unchanged = sources
            .documents
            .get(&physical.uri)
            .and_then(|source| source.document.pointer(&physical.pointer))
            .is_some_and(|original| original == &schema);
        if unchanged {
            self.import_with_dialect(
                Located {
                    value: schema,
                    address: physical,
                },
                dialect,
            )
        } else {
            self.import_serialized(schema, physical, sources, dialect)
        }
    }

    pub fn import_serialized(
        &mut self,
        schema: Value,
        physical: Address,
        sources: &mut Sources,
        dialect: SchemaDialect,
    ) -> Result<Value> {
        self.import_serialized_fragment(schema, physical, sources, dialect, "")
    }

    pub fn import_serialized_fragment(
        &mut self,
        schema: Value,
        physical: Address,
        sources: &mut Sources,
        dialect: SchemaDialect,
        pointer: &str,
    ) -> Result<Value> {
        let selected = schema.pointer(pointer).cloned().ok_or_else(|| {
            ConversionError::new(
                "RAML_SCHEMA_FRAGMENT",
                "Transformed JSON Schema fragment does not exist",
            )
        })?;
        let text = schema.to_string();
        let content_hash = hash(text.as_bytes());
        let name = format!(
            ".octri-schema-{}.json",
            &hash(format!("{}#{}:{content_hash}", physical.uri, physical.pointer).as_bytes())[..24]
        );
        let uri = sources.relative(&physical.uri, &name)?;
        if sources.files.contains_key(&uri) {
            return Err(ConversionError::new(
                "SCHEMA_DERIVED_URI",
                "Embedded schema identity collides with supplied source",
            ));
        }
        if let Some((previous, owner)) = self.derived.get(&uri) {
            if previous != &schema || owner != &physical {
                return Err(ConversionError::new(
                    "SCHEMA_DERIVED_URI",
                    "Embedded schema identity collision",
                ));
            }
        } else {
            let mut nodes = 0usize;
            let mut pending = vec![&schema];
            while let Some(value) = pending.pop() {
                nodes = nodes.saturating_add(1);
                if nodes > sources.limits.max_nodes {
                    return Err(ConversionError::new(
                        "SCHEMA_DERIVED_LIMIT",
                        "Embedded schema exceeds configured node limit",
                    ));
                }
                match value {
                    Value::Object(map) => pending.extend(map.values()),
                    Value::Array(values) => pending.extend(values),
                    _ => {}
                }
            }
            sources.admit_derived(&uri, text.len(), nodes)?;
            self.derived.insert(uri.clone(), (schema.clone(), physical));
        }
        if !pointer.is_empty() {
            self.register_context(
                Address {
                    uri: uri.clone(),
                    pointer: String::new(),
                },
                dialect,
            )?;
        }
        self.import_with_dialect(
            Located {
                value: selected,
                address: Address {
                    uri,
                    pointer: pointer.into(),
                },
            },
            dialect,
        )
    }

    pub fn import_with_dialect(&mut self, value: Located, dialect: SchemaDialect) -> Result<Value> {
        let dialect = value
            .value
            .get("$schema")
            .and_then(Value::as_str)
            .and_then(SchemaDialect::from_uri)
            .unwrap_or(dialect);
        if !value.value.is_object() && !value.value.is_boolean() {
            return Err(ConversionError::new(
                "SCHEMA_TYPE",
                "JSON Schema must be an object or boolean",
            ));
        }
        // Located already means physical. Remapping this pointer through a
        // logical alias can redirect numeric RAML declaration names twice.
        let address = value.address;
        let imported = self
            .imported
            .entry(address.clone())
            .or_insert_with(|| ImportedSchema {
                name: format!(
                    "Imported_{}",
                    &hash(format!("{}#{}", address.uri, address.pointer).as_bytes())[..16]
                ),
                dialect,
            });
        if imported.dialect != dialect {
            return Err(ConversionError::new(
                "SCHEMA_DIALECT_CONFLICT",
                "The same source schema was imported with conflicting default dialects",
            ));
        }
        Ok(json!({"$ref": format!("#{}", child("/components/schemas", &imported.name))}))
    }

    pub fn finish(
        self,
        sources: &mut Sources,
        mut definitions: Map<String, Value>,
    ) -> Result<Map<String, Value>> {
        if self.imported.is_empty() {
            return Ok(definitions);
        }
        for (address, imported) in &self.imported {
            let fragment = percent_encoding::utf8_percent_encode(
                &address.pointer,
                percent_encoding::NON_ALPHANUMERIC,
            );
            if definitions
                .insert(
                    imported.name.clone(),
                    json!({"$ref": format!("{}#{fragment}", address.uri)}),
                )
                .is_some()
            {
                return Err(ConversionError::new(
                    "SCHEMA_NAME_COLLISION",
                    "An authored schema name conflicts with a generated import name",
                ));
            }
        }
        let roots = self
            .imported
            .into_iter()
            .map(|(address, imported)| SchemaRoot {
                address,
                dialect: imported.dialect,
            })
            .collect();
        let mut registry = self.contexts;
        // Components make logical IDs available, but are emitted only when a
        // selected payload reaches them. Unused protocol schemas stay inert.
        for source in sources.documents.values() {
            if source.document.get("asyncapi").is_some() {
                for (name, schema) in source
                    .document
                    .pointer("/components/schemas")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(Map::iter)
                {
                    let Some(dialect) = super::asyncapi::schema_dialect(
                        schema.get("schemaFormat").and_then(Value::as_str),
                    ) else {
                        continue;
                    };
                    let pointer = child("/components/schemas", name);
                    let value = if schema.get("schemaFormat").is_some() {
                        schema.get("schema").unwrap_or(&Value::Null)
                    } else {
                        schema
                    };
                    if !value.is_object() && !value.is_boolean() {
                        continue;
                    }
                    let dialect = value
                        .get("$schema")
                        .and_then(Value::as_str)
                        .and_then(SchemaDialect::from_uri)
                        .unwrap_or(dialect);
                    registry.insert(
                        Address {
                            uri: source.uri.clone(),
                            pointer: if schema.get("schemaFormat").is_some()
                                && schema.get("schema").is_some()
                            {
                                child(&pointer, "schema")
                            } else {
                                pointer
                            },
                        },
                        dialect,
                    );
                }
            }
        }
        let envelope = json!({"openapi":"3.1.2","info":{"title":"Schema imports","version":"1"},"paths":{},"components":{"schemas":definitions}});
        let mut bundled = sources.bundle_derived_schemas(
            envelope,
            roots,
            registry
                .into_iter()
                .map(|(address, dialect)| SchemaRoot { address, dialect })
                .collect(),
            self.derived,
        )?;
        bundled
            .pointer_mut("/components/schemas")
            .and_then(Value::as_object_mut)
            .map(std::mem::take)
            .ok_or_else(|| {
                ConversionError::new("SCHEMA_BUNDLE", "Parser returned no schema registry")
            })
    }
}
