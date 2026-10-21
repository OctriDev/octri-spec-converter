//! Include selectors are typed metadata attached to their physical include site.
//! Template expansion may copy that site; it never embeds private marker strings.
use super::{schema_use::SchemaKind, xml_schemas::XmlSchemas};
use crate::{
    Classification, ConversionError, Result,
    adapters::references::{Located, Schemas},
    source::{SchemaFragment, Sources},
    trace::Trace,
};
use octri_parser::{Address, SchemaDialect};
use serde_json::Value;

pub(super) fn include(sources: &mut Sources, site: &Address, path: &str) -> Result<Option<Value>> {
    let mut target = url::Url::parse(&site.uri)
        .and_then(|base| base.join(path))
        .map_err(|error| ConversionError::new("INVALID_URI", error.to_string()))?;
    let Some(fragment) = target.fragment() else {
        return Ok(None);
    };
    let fragment = decode(fragment)?;
    target.set_fragment(None);
    let uri = sources.relative(&site.uri, target.as_str())?;
    let text = sources.text(&uri)?.to_owned();
    let header = text.trim();
    let header = header
        .strip_prefix('\u{feff}')
        .unwrap_or(header)
        .trim_start();
    let (selection, value) = if header.starts_with('<') {
        if !fragment.is_empty() && !crate::adapters::xml::ncname(&fragment) {
            return Err(invalid(
                "XML Schema fragment must name a global element or complex type",
            ));
        }
        (
            SchemaFragment::Xml {
                uri,
                name: (!fragment.is_empty()).then_some(fragment),
            },
            Value::String(text),
        )
    } else {
        if !header.starts_with('{') && !matches!(header, "true" | "false") {
            return Err(invalid(
                "Include fragments require a JSON or XML Schema document",
            ));
        }
        validate_pointer(&fragment)?;
        let document = sources.parse(&uri, false)?;
        let schema = document
            .pointer(&fragment)
            .ok_or_else(|| invalid("JSON Schema fragment does not exist"))?;
        if !schema.is_object() && !schema.is_boolean() {
            return Err(invalid(
                "JSON Schema fragment must select an object or boolean schema",
            ));
        }
        let value = if document.is_boolean() {
            document
        } else {
            Value::String(text)
        };
        (
            SchemaFragment::Json(Address {
                uri,
                pointer: fragment,
            }),
            value,
        )
    };
    sources.schema_fragments.insert(site.clone(), selection);
    Ok(Some(value))
}

pub(super) fn kind(site: &Address, sources: &Sources) -> Option<SchemaKind> {
    sources
        .schema_fragments
        .get(site)
        .map(|selection| match selection {
            SchemaFragment::Json(_) => SchemaKind::Json,
            SchemaFragment::Xml { .. } => SchemaKind::Xml,
        })
}

pub(super) fn import(
    site: &Address,
    expanded: &Value,
    sources: &mut Sources,
    schemas: &mut Schemas,
    xml_schemas: &mut XmlSchemas,
    trace: &mut Trace,
    default_dialect: SchemaDialect,
) -> Result<Option<Value>> {
    let Some(selection) = sources.schema_fragments.get(site).cloned() else {
        return Ok(None);
    };
    let value = (|| -> Result<Value> {
        Ok(match &selection {
            SchemaFragment::Json(address) => {
                let address = address.clone();
                let physical = Address {
                    uri: address.uri.clone(),
                    pointer: String::new(),
                };
                let original = sources.text(&address.uri)?;
                let rendered = expanded.as_str().filter(|rendered| *rendered != original);
                let document = if let Some(rendered) = rendered {
                    crate::adapters::references::parse_schema_text(rendered, &physical, sources)?
                } else {
                    sources.parse(&address.uri, false)?
                };
                let schema = document
                    .pointer(&address.pointer)
                    .cloned()
                    .ok_or_else(|| invalid("Selected JSON Schema is missing"))?;
                // The enclosing document owns dialect and identifier scope. Do not
                // clone the selected value into a new, incorrectly scoped resource.
                let dialect = match document.get("$schema") {
                    None => default_dialect,
                    Some(value) => value
                        .as_str()
                        .and_then(SchemaDialect::from_uri)
                        .ok_or_else(|| invalid("Enclosing JSON Schema dialect is unsupported"))?,
                };
                if rendered.is_some() {
                    return schemas.import_serialized_fragment(
                        document,
                        physical,
                        sources,
                        dialect,
                        &address.pointer,
                    );
                }
                schemas.register_context(
                    Address {
                        uri: address.uri.clone(),
                        pointer: String::new(),
                    },
                    dialect,
                )?;
                schemas.import_with_dialect(
                    Located {
                        value: schema,
                        address,
                    },
                    dialect,
                )?
            }
            SchemaFragment::Xml { uri, name } => {
                let physical = Address {
                    uri: uri.clone(),
                    pointer: String::new(),
                };
                let name = name.clone();
                let text = expanded
                    .as_str()
                    .unwrap_or(sources.text(&physical.uri)?)
                    .to_owned();
                xml_schemas.import_selected(&text, physical, name.as_deref(), sources, trace)?
            }
        })
    })()
    .map_err(|error| located(error, site, sources))?;
    trace.record(sources, &site.uri, &site.pointer, &[], "RAML_SCHEMA_FRAGMENT",
        Classification::Transformed,
        "Included schema fragment selected with its original resource scope; the complete source remains in the manifest", None);
    Ok(Some(value))
}

fn validate_pointer(pointer: &str) -> Result<()> {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(invalid(
            "JSON Schema fragment must be an RFC 6901 JSON Pointer",
        ));
    }
    let mut chars = pointer.chars();
    while let Some(ch) = chars.next() {
        if ch == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return Err(invalid("JSON Pointer contains an invalid tilde escape"));
        }
    }
    Ok(())
}

fn decode(fragment: &str) -> Result<String> {
    let mut bytes = fragment.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%'
            && !(bytes.next().is_some_and(|ch| ch.is_ascii_hexdigit())
                && bytes.next().is_some_and(|ch| ch.is_ascii_hexdigit()))
        {
            return Err(invalid(
                "Schema fragment contains an invalid percent escape",
            ));
        }
    }
    percent_encoding::percent_decode_str(fragment)
        .decode_utf8()
        .map(String::from)
        .map_err(|_| invalid("Schema fragment must be valid UTF-8"))
}

fn invalid(message: &str) -> ConversionError {
    ConversionError::new("RAML_SCHEMA_FRAGMENT", message)
}

pub(super) fn located(
    mut error: ConversionError,
    site: &Address,
    sources: &Sources,
) -> ConversionError {
    if error.diagnostics.is_empty() {
        error.diagnostics.push(octri_parser::Diagnostic {
            code: error.code.clone(),
            severity: octri_parser::Severity::Error,
            message: error.message.clone(),
            location: sources.physical_location(site),
            related: Vec::new(),
        });
    }
    error
}
