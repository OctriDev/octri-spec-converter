//! Traverse typed API positions. Schema instances and extension values stay inert.
use crate::source::child;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Document,
    Info,
    License,
    Components,
    PathItem,
    Operation,
    Parameter,
    RequestBody,
    Response,
    MediaType,
    Encoding,
    Header,
    Callback,
    Example,
    SecurityScheme,
    Flows,
    Flow,
    Tag,
    Server,
}
#[derive(Clone, Copy)]
enum Container {
    Object,
    Map,
    ExtensibleMap,
    Array,
}

pub(crate) fn schemas(document: &mut Value, visitor: &mut impl FnMut(&mut Value, &str)) {
    if let Some(map) = document
        .pointer_mut("/components/schemas")
        .and_then(Value::as_object_mut)
    {
        for (name, schema) in map {
            visitor(schema, &child("/components/schemas", name));
        }
    }
    structural(document, "", &mut |value, at, kind| {
        if matches!(kind, Kind::Parameter | Kind::Header | Kind::MediaType) {
            if let Some(schema) = value.get_mut("schema") {
                visitor(schema, &child(at, "schema"));
            }
            if let Some(schema) = value.get_mut("itemSchema") {
                visitor(schema, &child(at, "itemSchema"));
            }
        }
    });
}

pub(crate) fn structural(
    value: &mut Value,
    pointer: &str,
    visitor: &mut impl FnMut(&mut Value, &str, Kind),
) {
    walk(value, pointer, Kind::Document, visitor);
}

fn walk(
    value: &mut Value,
    pointer: &str,
    kind: Kind,
    visitor: &mut impl FnMut(&mut Value, &str, Kind),
) {
    use Container::{Array, ExtensibleMap, Map, Object};
    visitor(value, pointer, kind);
    let children: &[(&str, Kind, Container)] = match kind {
        Kind::Document => &[
            ("info", Kind::Info, Object),
            ("components", Kind::Components, Object),
            ("paths", Kind::PathItem, ExtensibleMap),
            ("webhooks", Kind::PathItem, Map),
            ("tags", Kind::Tag, Array),
            ("servers", Kind::Server, Array),
        ],
        Kind::Info => &[("license", Kind::License, Object)],
        Kind::Components => &[
            ("parameters", Kind::Parameter, Map),
            ("responses", Kind::Response, Map),
            ("requestBodies", Kind::RequestBody, Map),
            ("headers", Kind::Header, Map),
            ("callbacks", Kind::Callback, Map),
            ("examples", Kind::Example, Map),
            ("securitySchemes", Kind::SecurityScheme, Map),
            ("pathItems", Kind::PathItem, Map),
            ("mediaTypes", Kind::MediaType, Map),
        ],
        Kind::PathItem => &[
            ("parameters", Kind::Parameter, Array),
            ("get", Kind::Operation, Object),
            ("put", Kind::Operation, Object),
            ("post", Kind::Operation, Object),
            ("delete", Kind::Operation, Object),
            ("options", Kind::Operation, Object),
            ("head", Kind::Operation, Object),
            ("patch", Kind::Operation, Object),
            ("trace", Kind::Operation, Object),
            ("query", Kind::Operation, Object),
            ("additionalOperations", Kind::Operation, Map),
            ("servers", Kind::Server, Array),
        ],
        Kind::Operation => &[
            ("parameters", Kind::Parameter, Array),
            ("requestBody", Kind::RequestBody, Object),
            ("responses", Kind::Response, ExtensibleMap),
            ("callbacks", Kind::Callback, Map),
            ("servers", Kind::Server, Array),
        ],
        Kind::Parameter | Kind::Header => &[
            ("content", Kind::MediaType, Map),
            ("examples", Kind::Example, Map),
        ],
        Kind::RequestBody => &[("content", Kind::MediaType, Map)],
        Kind::Response => &[
            ("content", Kind::MediaType, Map),
            ("headers", Kind::Header, Map),
        ],
        Kind::MediaType => &[
            ("encoding", Kind::Encoding, Map),
            ("prefixEncoding", Kind::Encoding, Array),
            ("itemEncoding", Kind::Encoding, Object),
            ("examples", Kind::Example, Map),
        ],
        Kind::Encoding => &[
            ("headers", Kind::Header, Map),
            ("encoding", Kind::Encoding, Map),
            ("prefixEncoding", Kind::Encoding, Array),
            ("itemEncoding", Kind::Encoding, Object),
        ],
        Kind::SecurityScheme => &[("flows", Kind::Flows, Object)],
        Kind::Flows => &[
            ("implicit", Kind::Flow, Object),
            ("password", Kind::Flow, Object),
            ("clientCredentials", Kind::Flow, Object),
            ("authorizationCode", Kind::Flow, Object),
            ("deviceAuthorization", Kind::Flow, Object),
        ],
        _ => &[],
    };
    for (key, child_kind, container) in children {
        let Some(value) = value.get_mut(*key) else {
            continue;
        };
        let at = child(pointer, key);
        match container {
            Map | ExtensibleMap => {
                if let Some(map) = value.as_object_mut() {
                    for (name, value) in map {
                        if !matches!(container, ExtensibleMap) || !name.starts_with("x-") {
                            walk(value, &child(&at, name), *child_kind, visitor);
                        }
                    }
                }
            }
            Array => {
                if let Some(items) = value.as_array_mut() {
                    for (index, value) in items.iter_mut().enumerate() {
                        walk(value, &child(&at, &index.to_string()), *child_kind, visitor);
                    }
                }
            }
            Object => walk(value, &at, *child_kind, visitor),
        }
    }
    if kind == Kind::Callback
        && let Some(map) = value.as_object_mut()
    {
        for (name, value) in map {
            if !name.starts_with("x-") && name != "$ref" {
                walk(value, &child(pointer, name), Kind::PathItem, visitor);
            }
        }
    }
}
