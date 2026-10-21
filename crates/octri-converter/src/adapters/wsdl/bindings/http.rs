//! WSDL HTTP location, serialization, headers and fault projection.
use super::{
    AbstractOperation, Catalog, Endpoint, HTTP11, HTTP20, Kind, MIME, NodeRef, OperationBinding,
    WSDL11, WSDL20, base_operation, record, xsd,
};
use crate::adapters::http_method::HttpMethod;
use crate::{Classification, ConversionError, Result, source::Sources, trace::Trace};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Transport {
    Http11,
    Http20,
    /// SOAP-Response shares HTTP URI/form rules, with fixed GET and form
    /// serialization. Its response and faults remain SOAP envelopes.
    SoapResponse,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit(
    catalog: &Catalog,
    operation: &AbstractOperation,
    bound: &OperationBinding,
    transport: Transport,
    address: &url::Url,
    schemas: &Map<String, Value>,
    sources: &Sources,
    trace: &mut Trace,
) -> Result<Option<Endpoint>> {
    let modern = transport != Transport::Http11;
    let soap_response = transport == Transport::SoapResponse;
    let binding = &bound.binding;
    let source = bound.source();
    if modern
        && !matches!(
            catalog.node(&operation.declaration)?.attribute("pattern"),
            Some(
                "http://www.w3.org/ns/wsdl/in-out"
                    | "http://www.w3.org/ns/wsdl/in-only"
                    | "http://www.w3.org/ns/wsdl/robust-in-only"
            )
        )
    {
        return Err(ConversionError::new(
            "WSDL_HTTP_MEP",
            "HTTP binding requires an inbound WSDL message exchange pattern; provide explicit HTTP mapping for an extension pattern",
        ));
    }
    let (verb, location) = if soap_response {
        (
            "GET".into(),
            bound.attribute(catalog, HTTP20, "location")?.unwrap_or(""),
        )
    } else if modern {
        (
            bound
                .attribute(catalog, HTTP20, "method")?
                .or_else(|| {
                    catalog
                        .node(binding)
                        .ok()
                        .and_then(|node| node.attribute_ns(HTTP20, "methodDefault"))
                })
                .unwrap_or(
                    if catalog
                        .node(&operation.declaration)?
                        .attribute_ns("http://www.w3.org/ns/wsdl-extensions", "safe")
                        .is_some_and(|value| matches!(value, "true" | "1"))
                    {
                        "GET"
                    } else {
                        "POST"
                    },
                )
                .to_owned(),
            bound.attribute(catalog, HTTP20, "location")?.unwrap_or(""),
        )
    } else {
        let bindings = catalog.children(binding, HTTP11, "binding")?;
        let declaration = bindings.first().ok_or_else(|| {
            ConversionError::new("WSDL_HTTP_BINDING", "HTTP endpoint requires HTTP binding")
        })?;
        let verb = catalog
            .node(declaration)?
            .attribute("verb")
            .ok_or_else(|| ConversionError::new("WSDL_HTTP_METHOD", "HTTP binding requires verb"))?
            .to_owned();
        let operations = bound.children(catalog, HTTP11, "operation")?;
        let declaration = operations.first().ok_or_else(|| {
            ConversionError::new(
                "WSDL_HTTP_OPERATION",
                "HTTP binding operation requires declared location",
            )
        })?;
        let location = catalog
            .node(declaration)?
            .attribute("location")
            .ok_or_else(|| {
                ConversionError::new("WSDL_HTTP_LOCATION", "HTTP operation requires location")
            })?;
        (verb, location)
    };
    let method = HttpMethod::parse(&verb).ok_or_else(|| {
        ConversionError::new("WSDL_HTTP_METHOD", "Invalid declared HTTP method token")
    })?;
    // Resolve templates after replacing only their delimiters. URL normalization
    // must not percent-encode OpenAPI path parameters.
    let namespace = if modern { WSDL20 } else { WSDL11 };
    let inputs = bound.children(catalog, namespace, "input")?;
    let outputs = bound.children(catalog, namespace, "output")?;
    let replacement = !modern
        && inputs
            .first()
            .map(|input| catalog.children(input, HTTP11, "urlReplacement"))
            .transpose()?
            .is_some_and(|values| !values.is_empty());
    let location = if !replacement {
        location.to_owned()
    } else {
        location.replace('(', "{").replace(')', "}")
    };
    let path = request_path(address, &location)?;
    // Keep existing identifiers for fixed OpenAPI methods. Custom wire tokens
    // retain their spelling and must never alias a case-different method.
    let identity = match method {
        HttpMethod::Fixed(field) => field.to_owned(),
        HttpMethod::Additional(wire) => format!("additionalOperations/{wire}"),
    };
    let mut emitted = base_operation(operation, address, &identity);
    let input_serialization = if soap_response {
        "application/x-www-form-urlencoded".into()
    } else if modern {
        bound
            .attribute(catalog, HTTP20, "inputSerialization")?
            .unwrap_or(if matches!(verb.as_str(), "GET" | "DELETE") {
                "application/x-www-form-urlencoded"
            } else {
                "application/xml"
            })
            .to_owned()
    } else if let Some(input) = inputs.first() {
        mime(catalog, input)?.unwrap_or_else(|| {
            if verb == "GET" {
                "application/x-www-form-urlencoded"
            } else {
                "application/xml"
            }
            .into()
        })
    } else {
        "application/xml".into()
    };
    let mut input = operation.input.clone();
    if let Some(schema) = input.take() {
        let expanded = expand_object(schema, schemas, sources.limits.max_depth)?;
        let mut remaining = expanded;
        let mut parameters = Vec::new();
        let properties = remaining
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let names = path
            .split('{')
            .skip(1)
            .map(|part| {
                part.split_once('}')
                    .map(|(name, _)| name.to_owned())
                    .ok_or_else(|| {
                        ConversionError::new("WSDL_HTTP_TEMPLATE", "Unclosed path template")
                    })
            })
            .collect::<Result<BTreeSet<_>>>()?;
        for name in &names {
            let property = properties.get(name).ok_or_else(|| {
                ConversionError::new(
                    "WSDL_HTTP_TEMPLATE",
                    format!("Path template {name} does not identify an input property"),
                )
            })?;
            parameters.push(json!({"name":name,"in":"path","required":true,"schema":property}));
            if let Some(map) = remaining
                .get_mut("properties")
                .and_then(Value::as_object_mut)
            {
                map.remove(name);
            }
        }
        if replacement && properties.keys().any(|name| !names.contains(name)) {
            record(
                catalog,
                source,
                sources,
                trace,
                "WSDL_URL_REPLACEMENT_UNUSED_PART",
                Classification::Omitted,
                "urlReplacement leaves uncited message parts out of the request URI; they remain in original source.",
            );
        }
        if remaining["type"] != "object"
            && properties.is_empty()
            && input_serialization == "application/x-www-form-urlencoded"
        {
            record(
                catalog,
                source,
                sources,
                trace,
                "WSDL_HTTP_FORM_INPUT",
                Classification::Blocked,
                "Form/query serialization requires a concrete object of declared message parts; provide explicit HTTP mapping for scalar or opaque input.",
            );
        }
        if replacement {
            // urlReplacement serializes cited parts only, without a body or
            // additional query parameters.
        } else if matches!(verb.as_str(), "GET" | "HEAD" | "DELETE")
            && input_serialization == "application/x-www-form-urlencoded"
        {
            for (name, schema) in properties.iter().filter(|(name, _)| !names.contains(*name)) {
                let required = remaining["required"]
                    .as_array()
                    .is_some_and(|values| values.contains(&json!(name)));
                parameters
                    .push(json!({"name":name,"in":"query","required":required,"schema":schema}));
            }
        } else if !remaining["properties"]
            .as_object()
            .is_some_and(Map::is_empty)
        {
            if let Some(required) = remaining.get_mut("required").and_then(Value::as_array_mut) {
                required.retain(|name| name.as_str().is_none_or(|name| !names.contains(name)));
            }
            emitted["requestBody"] =
                json!({"required":true,"content":{&input_serialization:{"schema":remaining}}});
        }
        if !parameters.is_empty() {
            emitted["parameters"] = Value::Array(parameters);
        }
    }
    let success = if modern {
        match catalog.node(&operation.declaration)?.attribute("pattern") {
            Some("http://www.w3.org/ns/wsdl/in-only") => "202",
            Some("http://www.w3.org/ns/wsdl/robust-in-only") => "204",
            _ => "2XX",
        }
    } else {
        "2XX"
    };
    emitted["responses"][success] = json!({"description":"Declared HTTP success"});
    if let Some(output) = &operation.output {
        let alternatives = if modern {
            vec![(
                bound
                    .attribute(catalog, HTTP20, "outputSerialization")?
                    .unwrap_or("application/xml")
                    .to_owned(),
                None,
            )]
        } else if let Some(output) = outputs.first() {
            mime_alternatives(catalog, output)?
        } else {
            Vec::new()
        };
        let alternatives = if alternatives.is_empty() {
            vec![("application/xml".into(), None)]
        } else {
            alternatives
        };
        for (media, part) in alternatives {
            let schema = if let Some(part) = part {
                let references = catalog.children(&operation.declaration, namespace, "output")?;
                let reference = references.first().ok_or_else(|| {
                    ConversionError::new("WSDL_MIME_PART", "MIME output has no abstract message")
                })?;
                super::soap::message_parts(catalog, reference, Some(&part))?
                    .unwrap_or_else(|| json!({}))
            } else {
                output.clone()
            };
            emitted["responses"][success]["content"][media] = json!({"schema":schema});
        }
    }
    if modern {
        http20_headers(catalog, &inputs, &outputs, &mut emitted, success)?;
        if !soap_response {
            http20_faults(catalog, operation, bound, &mut emitted)?;
        }
    }
    Ok(Some(Endpoint {
        operation: emitted,
        method: verb,
        path,
        source: source.clone(),
    }))
}

pub(super) fn request_path(address: &url::Url, location: &str) -> Result<String> {
    let url = address
        .join(location)
        .map_err(|error| ConversionError::new("WSDL_HTTP_LOCATION", error.to_string()))?;
    if url.origin() != address.origin() || url.query().is_some() || url.fragment().is_some() {
        return Err(ConversionError::new(
            "WSDL_HTTP_LOCATION",
            "Operation location cannot change endpoint origin or embed query/fragment; provide explicit HTTP mapping",
        ));
    }
    Ok(url.path().replace("%7B", "{").replace("%7D", "}"))
}

fn http20_headers(
    catalog: &Catalog,
    inputs: &[NodeRef],
    outputs: &[NodeRef],
    operation: &mut Value,
    success: &str,
) -> Result<()> {
    for (references, input) in [(inputs, true), (outputs, false)] {
        for reference in references {
            for header in catalog.children(reference, HTTP20, "header")? {
                let node = catalog.node(&header)?;
                let name = node.attribute("name").ok_or_else(|| {
                    ConversionError::new("WSDL_HTTP_HEADER", "HTTP header requires name")
                })?;
                if name.is_empty()
                    || !name
                        .bytes()
                        .all(|ch| ch.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&ch))
                {
                    return Err(ConversionError::new(
                        "WSDL_HTTP_HEADER",
                        "Header name must be an HTTP token",
                    ));
                }
                let kind = node.attribute("type").ok_or_else(|| {
                    ConversionError::new("WSDL_HTTP_HEADER", "HTTP header requires type QName")
                })?;
                let schema = xsd::reference(catalog, Kind::Type, &node.qname(kind)?)?;
                let required = node
                    .attribute("required")
                    .is_some_and(|value| matches!(value, "true" | "1"));
                if input {
                    if operation.get("parameters").is_none() {
                        operation["parameters"] = json!([]);
                    }
                    if let Some(parameters) = operation["parameters"].as_array_mut() {
                        parameters.push(
                            json!({"name":name,"in":"header","required":required,"schema":schema}),
                        );
                    }
                } else {
                    operation["responses"][success]["headers"][name] =
                        json!({"required":required,"schema":schema});
                }
            }
        }
    }
    Ok(())
}

fn http20_faults(
    catalog: &Catalog,
    operation: &AbstractOperation,
    bound: &OperationBinding,
    emitted: &mut Value,
) -> Result<()> {
    let declared = catalog.interface_members(&operation.interface, "fault")?;
    let bound_faults = catalog.children(&bound.binding, WSDL20, "fault")?;
    let media = bound
        .attribute(catalog, HTTP20, "faultSerialization")?
        .unwrap_or("application/xml");
    for fault in catalog.children(&operation.declaration, WSDL20, "outfault")? {
        let node = catalog.node(&fault)?;
        let reference = node
            .attribute("ref")
            .ok_or_else(|| ConversionError::new("WSDL_FAULT", "outfault requires ref QName"))?;
        let name = node.qname(reference)?;
        let definition = declared
            .iter()
            .find(|(owner, at)| {
                owner.namespace == name.namespace
                    && catalog
                        .node(at)
                        .is_ok_and(|node| node.attribute("name") == Some(name.local.as_str()))
            })
            .ok_or_else(|| {
                ConversionError::new("WSDL_FAULT", "Fault reference has no interface declaration")
            })?;
        let definition = catalog.node(&definition.1)?;
        let element = definition.attribute("element").ok_or_else(|| {
            ConversionError::new("WSDL_FAULT", "Fault declaration requires element QName")
        })?;
        let schema = xsd::reference(catalog, Kind::Element, &definition.qname(element)?)?;
        let mut code = "default";
        for candidate in &bound_faults {
            let candidate = catalog.node(candidate)?;
            if candidate
                .attribute("ref")
                .map(|reference| candidate.qname(reference))
                .transpose()?
                .as_ref()
                == Some(&name)
            {
                code = candidate
                    .attribute_ns(HTTP20, "code")
                    .filter(|code| *code != "#any")
                    .unwrap_or("default");
                break;
            }
        }
        if code != "default"
            && (!crate::adapters::http::valid_status(code) || !code.starts_with(['4', '5']))
        {
            return Err(ConversionError::new(
                "WSDL_FAULT_STATUS",
                "HTTP fault status must identify a 4xx or 5xx response",
            ));
        }
        if let Some(previous) = emitted["responses"][code]["content"][media].get_mut("schema") {
            *previous = json!({"anyOf":[previous.clone(),schema]});
        } else {
            emitted["responses"][code] = json!({"description":format!("Declared WSDL fault {}",name.local),"content":{media:{"schema":schema}}});
        }
    }
    Ok(())
}

fn mime(catalog: &Catalog, at: &NodeRef) -> Result<Option<String>> {
    if let Some(content) = catalog.children(at, MIME, "content")?.first() {
        return Ok(catalog.node(content)?.attribute("type").map(str::to_owned));
    }
    if !catalog.children(at, HTTP11, "urlEncoded")?.is_empty() {
        return Ok(Some("application/x-www-form-urlencoded".into()));
    }
    if !catalog.children(at, MIME, "mimeXml")?.is_empty() {
        return Ok(Some("application/xml".into()));
    }
    Ok(None)
}

fn mime_alternatives(catalog: &Catalog, at: &NodeRef) -> Result<Vec<(String, Option<String>)>> {
    let mut result = Vec::new();
    for content in catalog.children(at, MIME, "content")? {
        let node = catalog.node(&content)?;
        result.push((
            node.attribute("type").unwrap_or("*/*").into(),
            node.attribute("part").map(str::to_owned),
        ));
    }
    for xml in catalog.children(at, MIME, "mimeXml")? {
        let node = catalog.node(&xml)?;
        result.push((
            "application/xml".into(),
            node.attribute("part").map(str::to_owned),
        ));
    }
    Ok(result)
}

fn expand_object(mut schema: Value, schemas: &Map<String, Value>, limit: usize) -> Result<Value> {
    let mut seen = BTreeSet::new();
    while let Some(reference) = schema
        .get("$ref")
        .and_then(Value::as_str)
        .map(str::to_owned)
    {
        if !seen.insert(reference.clone()) || seen.len() > limit {
            return Err(ConversionError::new(
                "WSDL_HTTP_INPUT",
                "Input schema reference cycle has no concrete object",
            ));
        }
        let name = reference
            .strip_prefix("#/components/schemas/")
            .ok_or_else(|| {
                ConversionError::new(
                    "WSDL_HTTP_INPUT",
                    "HTTP input schema must reference generated components",
                )
            })?
            .replace("~1", "/")
            .replace("~0", "~");
        let target = schemas.get(&name).ok_or_else(|| {
            ConversionError::new("WSDL_HTTP_INPUT", "HTTP input schema component is missing")
        })?;
        let mut expanded = target.clone();
        if let Some(map) = schema.as_object() {
            for (key, value) in map {
                if key != "$ref" {
                    expanded[key] = value.clone();
                }
            }
        }
        schema = expanded;
    }
    Ok(schema)
}
