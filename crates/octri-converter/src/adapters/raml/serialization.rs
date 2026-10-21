//! RAML serialization declarations are distinct from JSON instance constraints.
//! Preserve metadata without claiming that OpenAPI runs an XML/file serializer.
use crate::{
    Classification, ConversionError, Result,
    source::{Sources, child},
    trace::Trace,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Scalar,
    File,
    Array,
    Object,
    Unknown,
}

pub(super) fn apply(
    schema: &mut Value,
    declaration: &Value,
    pointer: &str,
    declarations: &Map<String, Value>,
    sources: &Sources,
    trace: &mut Trace,
) -> Result<()> {
    if ["xml", "fileTypes", "minLength", "maxLength"]
        .iter()
        .all(|key| declaration.get(key).is_none())
    {
        return Ok(());
    }
    let shape = shape(
        declaration,
        declarations,
        &mut BTreeSet::new(),
        sources.limits.max_depth,
    );
    if let Some(xml) = declaration.get("xml") {
        xml_metadata(schema, xml, shape, pointer, sources, trace)?;
    }
    if shape == Shape::File {
        // RAML file lengths count bytes. JSON Schema string lengths count code
        // points and would assert a different contract, including on aliases.
        let mut limits = Map::new();
        for key in ["minLength", "maxLength"] {
            if let Some(value) = declaration.get(key) {
                if !value.as_number().is_some_and(|number| {
                    octri_parser::numbers::is_integer(number)
                        && octri_parser::numbers::compare_text(&number.to_string(), "0").is_ge()
                }) {
                    return Err(invalid(
                        sources,
                        &child(pointer, key),
                        "File byte limits must be nonnegative integers",
                    ));
                }
                if let Some(map) = schema.as_object_mut() {
                    map.remove(key);
                }
                limits.insert(key.into(), value.clone());
            }
        }
        if let (Some(minimum), Some(maximum)) = (
            limits.get("minLength").and_then(Value::as_number),
            limits.get("maxLength").and_then(Value::as_number),
        ) && octri_parser::numbers::compare(minimum, maximum).is_gt()
        {
            return Err(invalid(
                sources,
                pointer,
                "File minimum byte length cannot exceed maximum byte length",
            ));
        }
        if !limits.is_empty() {
            schema["x-raml-file-byte-length"] = Value::Object(limits);
            record(
                sources,
                trace,
                pointer,
                "RAML_FILE_BYTE_LENGTH",
                Classification::Inferred,
                "File byte limits retained as serialization metadata. Enforce them on decoded file bytes in the receiving runtime; JSON string length is not equivalent.",
                declaration,
            );
        }
    }
    if let Some(media) = declaration.get("fileTypes") {
        if shape != Shape::File {
            return Err(invalid(
                sources,
                &child(pointer, "fileTypes"),
                "fileTypes requires a file type",
            ));
        }
        let values = media
            .as_array()
            .filter(|values| !values.is_empty())
            .ok_or_else(|| {
                invalid(
                    sources,
                    &child(pointer, "fileTypes"),
                    "fileTypes must be a nonempty array of media types",
                )
            })?;
        for value in values {
            if !value.as_str().is_some_and(media_type) {
                return Err(invalid(
                    sources,
                    &child(pointer, "fileTypes"),
                    "fileTypes entries must be media types or type wildcards",
                ));
            }
        }
        schema["x-raml-file-types"] = media.clone();
        record(
            sources,
            trace,
            &child(pointer, "fileTypes"),
            "RAML_FILE_TYPES",
            Classification::Inferred,
            "Allowed file media types retained as serialization metadata. Review receiving runtime enforcement; OpenAPI content annotations do not enforce uploaded file types.",
            media,
        );
    }
    Ok(())
}

fn xml_metadata(
    schema: &mut Value,
    xml: &Value,
    shape: Shape,
    pointer: &str,
    sources: &Sources,
    trace: &mut Trace,
) -> Result<()> {
    let at = child(pointer, "xml");
    let map = xml
        .as_object()
        .ok_or_else(|| invalid(sources, &at, "xml must be an object"))?;
    for key in ["attribute", "wrapped"] {
        if map.get(key).is_some_and(|value| !value.is_boolean()) {
            return Err(invalid(
                sources,
                &child(&at, key),
                "XML flags must be booleans",
            ));
        }
    }
    for key in ["name", "namespace", "prefix"] {
        if map.get(key).is_some_and(|value| !value.is_string()) {
            return Err(invalid(
                sources,
                &child(&at, key),
                "XML names, namespaces and prefixes must be strings",
            ));
        }
    }
    let attribute = map
        .get("attribute")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let wrapped = map.get("wrapped").and_then(Value::as_bool).unwrap_or(false);
    if attribute && (wrapped || matches!(shape, Shape::Array | Shape::Object | Shape::File))
        || wrapped && shape == Shape::Scalar
    {
        return Err(invalid(
            sources,
            &at,
            "XML attribute requires a scalar type; wrapped cannot be true on a scalar or attribute",
        ));
    }
    let mut output = Map::new();
    let mut retained = Map::new();
    for (key, value) in map {
        let representable = match key.as_str() {
            "attribute" | "name" | "prefix" => true,
            "wrapped" => shape == Shape::Array,
            "namespace" => value
                .as_str()
                .is_some_and(|value| url::Url::parse(value).is_ok()),
            _ => false,
        };
        if representable {
            output.insert(key.clone(), value.clone());
        } else {
            retained.insert(key.clone(), value.clone());
        }
    }
    if attribute && shape == Shape::Unknown {
        retained.insert("attribute".into(), json!(true));
        output.remove("attribute");
    }
    if shape == Shape::Array && output.contains_key("wrapped") {
        // OpenAPI's wrapped flag takes effect only beside an explicit array
        // type, even when RAML inherited that type through a named reference.
        schema["type"] = json!("array");
        // Swagger 2.0 requires an items field on every explicit array. An
        // unconstrained sibling keeps the referenced RAML item constraints;
        // it does not copy or replace the inherited schema.
        if schema.get("items").is_none() {
            schema["items"] = json!({});
        }
    }
    if !output.is_empty() {
        schema["xml"] = Value::Object(output);
    }
    if !retained.is_empty() {
        schema["x-raml-xml"] = Value::Object(retained);
        record(
            sources,
            trace,
            &at,
            "RAML_XML_SERIALIZATION",
            Classification::Inferred,
            "XML metadata without an equivalent OpenAPI XML field retained separately. Review object wrapping, relative namespaces, custom fields or unresolved scalar type before selecting a serializer.",
            xml,
        );
    }
    record(
        sources,
        trace,
        &at,
        "RAML_XML_METADATA",
        Classification::Transformed,
        "Declared XML names, namespace, prefix and representable attribute/array wrapping metadata preserved; JSON instance constraints remain separate.",
        xml,
    );
    Ok(())
}

fn shape(
    value: &Value,
    declarations: &Map<String, Value>,
    seen: &mut BTreeSet<String>,
    remaining: usize,
) -> Shape {
    if remaining == 0 {
        return Shape::Unknown;
    }
    if let Some(map) = value.as_object() {
        return map.get("type").or_else(|| map.get("schema")).map_or_else(
            || {
                if map.contains_key("properties") {
                    Shape::Object
                } else if map.contains_key("items") {
                    Shape::Array
                } else {
                    Shape::Scalar
                }
            },
            |base| shape(base, declarations, seen, remaining - 1),
        );
    }
    if let Some(bases) = value.as_array() {
        return common_shape(
            bases
                .iter()
                .map(|base| shape(base, declarations, seen, remaining - 1)),
        );
    }
    let Some(expression) = value.as_str() else {
        return Shape::Unknown;
    };
    let expression = expression.trim();
    let Ok(alternatives) = super::types::split_union(expression) else {
        return Shape::Unknown;
    };
    if alternatives.len() > 1 {
        return common_shape(
            alternatives
                .into_iter()
                .map(|part| shape(&json!(part), declarations, seen, remaining - 1)),
        );
    }
    if expression.ends_with("[]") {
        return Shape::Array;
    }
    if expression.starts_with('(') && expression.ends_with(')') {
        return shape(
            &json!(&expression[1..expression.len() - 1]),
            declarations,
            seen,
            remaining - 1,
        );
    }
    match expression {
        "file" => Shape::File,
        "array" => Shape::Array,
        "object" => Shape::Object,
        "string" | "boolean" | "integer" | "number" | "nil" | "date-only" | "time-only"
        | "datetime" | "datetime-only" => Shape::Scalar,
        name => {
            if !seen.insert(name.into()) {
                return Shape::Unknown;
            }
            let result = declarations.get(name).map_or(Shape::Unknown, |value| {
                shape(value, declarations, seen, remaining - 1)
            });
            seen.remove(name);
            result
        }
    }
}

fn common_shape(mut shapes: impl Iterator<Item = Shape>) -> Shape {
    let first = shapes.next().unwrap_or(Shape::Unknown);
    if shapes.all(|shape| shape == first) {
        first
    } else {
        Shape::Unknown
    }
}

fn media_type(value: &str) -> bool {
    let Some((kind, subtype)) = value.split_once('/') else {
        return false;
    };
    let token = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
    };
    token(kind)
        && token(subtype)
        && (kind != "*" || subtype == "*")
        && (!kind.contains('*') || kind == "*")
        && (!subtype.contains('*') || subtype == "*")
}

fn invalid(sources: &Sources, pointer: &str, message: &str) -> ConversionError {
    ConversionError {
        code: "RAML_SERIALIZATION".into(),
        message: message.into(),
        diagnostics: vec![octri_parser::Diagnostic {
            code: "RAML_SERIALIZATION".into(),
            severity: octri_parser::Severity::Error,
            message: message.into(),
            location: sources.location(&sources.root, pointer),
            related: Vec::new(),
        }],
    }
}

fn record(
    sources: &Sources,
    trace: &mut Trace,
    pointer: &str,
    code: &str,
    class: Classification,
    message: &str,
    evidence: &Value,
) {
    trace.record(
        sources,
        &sources.root,
        pointer,
        &[],
        code,
        class,
        message,
        Some(evidence),
    );
}
