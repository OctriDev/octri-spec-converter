//! SOAP envelope, action, header and fault projection.
use super::{
    AbstractOperation, Catalog, Endpoint, Kind, NodeRef, OperationBinding, SOAP11, SOAP20, WSDL11,
    WSDL20, base_operation, part_schema, record, xsd,
};
use crate::{Classification, ConversionError, Result, source::Sources, trace::Trace};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit(
    catalog: &Catalog,
    operation: &AbstractOperation,
    bound: &OperationBinding,
    protocol: &str,
    address: &url::Url,
    schemas: &Map<String, Value>,
    sources: &Sources,
    trace: &mut Trace,
) -> Result<Option<Endpoint>> {
    let modern = protocol == SOAP20;
    let binding = &bound.binding;
    let source = bound.source();
    let version12 = protocol != SOAP11;
    let namespace = if modern { WSDL20 } else { WSDL11 };
    if !modern {
        let bindings = catalog.children(binding, protocol, "binding")?;
        let Some(binding_declaration) = bindings.first() else {
            return Err(ConversionError::new(
                "WSDL_SOAP_BINDING",
                "SOAP address requires matching SOAP binding",
            ));
        };
        if catalog.node(binding_declaration)?.attribute("transport")
            != Some("http://schemas.xmlsoap.org/soap/http")
        {
            return Ok(None);
        }
    } else if catalog.node(binding)?.attribute_ns(SOAP20, "protocol")
        != Some("http://www.w3.org/2003/05/soap/bindings/HTTP/")
    {
        return Ok(None);
    }
    let soap_response = modern && response_mep(catalog, operation, bound)?;
    if modern
        && catalog
            .node(binding)?
            .attribute_ns(SOAP20, "version")
            .unwrap_or("1.2")
            != "1.2"
    {
        return Err(ConversionError::new(
            "WSDL_SOAP_VERSION",
            "WSDL 2.0 native projection requires SOAP 1.2; supply an explicit HTTP mapping for another version",
        ));
    }
    let bindings = catalog.children(binding, protocol, "binding")?;
    let concrete = bound.children(catalog, protocol, "operation")?;
    let concrete_node = concrete.first().map(|at| catalog.node(at)).transpose()?;
    let style = concrete_node
        .and_then(|node| node.attribute("style"))
        .or_else(|| {
            bindings
                .first()
                .and_then(|at| catalog.node(at).ok())
                .and_then(|node| node.attribute("style"))
        })
        .unwrap_or("document");
    let action = if soap_response {
        None // SOAP Action has no effect for the SOAP-Response MEP.
    } else if modern {
        bound.attribute(catalog, SOAP20, "action")?
    } else {
        concrete_node.and_then(|node| node.attribute("soapAction"))
    };
    let envelope_namespace = if version12 {
        "http://www.w3.org/2003/05/soap-envelope"
    } else {
        "http://schemas.xmlsoap.org/soap/envelope/"
    };
    let media = if version12 {
        "application/soap+xml"
    } else {
        "text/xml"
    };
    let (mut emitted, path) = if soap_response {
        let endpoint = super::http::emit(
            catalog,
            operation,
            bound,
            super::http::Transport::SoapResponse,
            address,
            schemas,
            sources,
            trace,
        )?
        .ok_or_else(|| {
            ConversionError::new("WSDL_SOAP_RESPONSE", "SOAP response has no HTTP projection")
        })?;
        (endpoint.operation, endpoint.path)
    } else {
        let path = if modern {
            let location = bound
                .attribute(catalog, super::HTTP20, "location")?
                .unwrap_or("");
            if location.contains(['{', '}']) {
                return Err(ConversionError::new(
                    "WSDL_SOAP_URI_TEMPLATE",
                    "SOAP request-response URI templates require explicit HTTP mappings for body and path serialization",
                ));
            }
            super::http::request_path(address, location)?
        } else {
            address.path().to_owned()
        };
        (base_operation(operation, address, "post"), path)
    };
    if !version12 {
        if let Some(action) = action {
            emitted["parameters"] = json!([{"name":"SOAPAction","in":"header","required":true,"schema":{"type":"string","enum":[format!("\"{action}\"")]}}]);
        } else {
            record(
                catalog,
                source,
                sources,
                trace,
                "WSDL_SOAP_ACTION",
                Classification::Blocked,
                "SOAP 1.1 HTTP operation requires declared soapAction; supply explicit HTTP mapping when absent",
            );
        }
    } else if let Some(action) = action {
        emitted["x-soap-action"] = json!(action);
        record(
            catalog,
            source,
            sources,
            trace,
            "SOAP12_ACTION_PARAMETER",
            Classification::Omitted,
            "SOAP 1.2 action is a Content-Type media parameter. Preserved as x-soap-action; OpenAPI consumers must configure serializer to send it.",
        );
    }
    let mut input = operation.input.clone();
    let mut output = operation.output.clone();
    for (direction, payload) in [("input", &mut input), ("output", &mut output)] {
        if soap_response && direction == "input" {
            for message in bound.children(catalog, namespace, direction)? {
                if !catalog.children(&message, protocol, "header")?.is_empty() {
                    return Err(ConversionError::new(
                        "SOAP_HEADER",
                        "SOAP-Response has no outbound SOAP message for input headers; provide explicit HTTP mapping",
                    ));
                }
            }
            // Input was serialized into the request URI by the HTTP rules.
            *payload = None;
            continue;
        }
        let abstract_messages = catalog.children(&operation.declaration, namespace, direction)?;
        let concrete_messages = bound.children(catalog, namespace, direction)?;
        let mut headers = HeaderBlocks::default();
        if let Some(message) = concrete_messages.first() {
            if let Some(body) = catalog.children(message, protocol, "body")?.first() {
                let body_node = catalog.node(body)?;
                if body_node.attribute("use") == Some("encoded")
                    || body_node.attribute("encodingStyle").is_some()
                {
                    record(
                        catalog,
                        body,
                        sources,
                        trace,
                        "SOAP_ENCODED_MESSAGE",
                        Classification::Omitted,
                        "SOAP encoding graph identity, array rules and xsi:type behavior remain in original binding. OpenAPI projection describes payload shape only.",
                    );
                }
                if let (Some(parts), Some(abstract_message)) =
                    (body_node.attribute("parts"), abstract_messages.first())
                {
                    *payload = message_parts(catalog, abstract_message, Some(parts))?;
                }
                if style == "rpc" {
                    let node = catalog.node(&operation.declaration)?;
                    let name = node.attribute("name").unwrap_or("Operation");
                    let wrapper = if direction == "output" {
                        format!("{name}Response")
                    } else {
                        name.into()
                    };
                    if let Some(payload) = payload {
                        payload["xml"] = json!({"name":wrapper,"namespace":body_node.attribute("namespace").unwrap_or(&operation.interface.namespace)});
                    }
                }
            }
            for header in catalog.children(message, protocol, "header")? {
                if modern {
                    element_header(catalog, &header, &mut headers)?;
                    continue;
                }
                let header_node = catalog.node(&header)?;
                let message_name = header_node.attribute("message").ok_or_else(|| {
                    ConversionError::new("SOAP_HEADER", "SOAP header requires message QName")
                })?;
                let part = header_node.attribute("part").ok_or_else(|| {
                    ConversionError::new("SOAP_HEADER", "SOAP header requires part")
                })?;
                let declaration =
                    catalog.declaration(Kind::Message, &header_node.qname(message_name)?)?;
                let parts = catalog.children(&declaration.node, WSDL11, "part")?;
                let header_part = parts
                    .iter()
                    .find(|at| {
                        catalog
                            .node(at)
                            .is_ok_and(|node| node.attribute("name") == Some(part))
                    })
                    .ok_or_else(|| {
                        ConversionError::new("SOAP_HEADER", "SOAP header part does not exist")
                    })?;
                headers.schemas.insert(
                    part.into(),
                    part_schema(catalog, catalog.node(header_part)?)?,
                );
                record(
                    catalog,
                    &header,
                    sources,
                    trace,
                    "SOAP_HEADER_PROCESSING",
                    Classification::Omitted,
                    "SOAP header schema retained. mustUnderstand, actor/role, headerfault and processing lifecycle require SOAP-aware client behavior.",
                );
            }
        }
        if !abstract_messages.is_empty() {
            *payload = Some(envelope(payload.take(), headers, envelope_namespace));
        }
    }
    if let Some(input) = input {
        emitted["requestBody"] = json!({"required":true,"content":{media:{"schema":input}}});
    }
    emitted["responses"]["2XX"] = if let Some(output) = output {
        json!({"description":"SOAP success response","content":{media:{"schema":output}}})
    } else {
        json!({"description":"SOAP message accepted"})
    };
    let faults = soap_faults(catalog, operation, namespace)?;
    let fault_schema = if modern && !faults.is_empty() {
        let headers = fault_headers(catalog, bound)?;
        let application_details = faults
            .iter()
            .map(|(_, schema)| schema.clone())
            .collect::<Vec<_>>();
        let mut alternatives = faults
            .into_iter()
            .map(|(name, schema)| {
                let headers = name
                    .as_ref()
                    .and_then(|name| headers.get(name))
                    .cloned()
                    .unwrap_or_default();
                let mut envelope = fault_envelope(
                    json!({"oneOf":[schema]}),
                    headers,
                    version12,
                    envelope_namespace,
                );
                envelope["properties"]["Body"]["properties"]["payload"]["required"] =
                    json!(["Code", "Reason", "Detail"]);
                envelope
            })
            .collect::<Vec<_>>();
        // Protocol faults do not inherit application-fault header requirements.
        // Exclude declared application detail shapes from this fallback so it
        // cannot bypass the corresponding application's required headers.
        alternatives.push(fault_envelope(
            json!({"not":{"anyOf":application_details}}),
            HeaderBlocks::default(),
            version12,
            envelope_namespace,
        ));
        record(
            catalog,
            source,
            sources,
            trace,
            "SOAP_FAULT_DETAIL_DISPATCH",
            Classification::Omitted,
            "Application faults retain per-fault headers and protocol faults remain alternatives. OpenAPI schema validation distinguishes detail shapes; SOAP QName/code dispatch and overlapping XML detail types require a SOAP-aware serializer.",
        );
        json!({"anyOf":alternatives})
    } else {
        // Keep WSDL 1.1's message-part projection and generic protocol fault.
        // Validate WSDL 2.0 binding fault declarations even when this particular
        // operation does not reference an application fault.
        if modern {
            fault_headers(catalog, bound)?;
        }
        let detail = if faults.is_empty() {
            json!({})
        } else {
            json!({"oneOf":faults.into_iter().map(|(_,schema)|schema).collect::<Vec<_>>()})
        };
        fault_envelope(
            detail,
            HeaderBlocks::default(),
            version12,
            envelope_namespace,
        )
    };
    emitted["responses"]["500"] =
        json!({"description":"SOAP fault","content":{media:{"schema":fault_schema}}});
    emitted["x-wsdl-soap"] =
        json!({"version":if version12 {"1.2"}else{"1.1"},"style":style,"action":action});
    Ok(Some(Endpoint {
        operation: emitted,
        method: if soap_response { "GET" } else { "POST" }.into(),
        path,
        source: source.clone(),
    }))
}

fn fault_envelope(detail: Value, headers: HeaderBlocks, version12: bool, namespace: &str) -> Value {
    let fault = if version12 {
        json!({"type":"object","xml":{"name":"Fault","namespace":namespace},"properties":{"Code":{"type":"object"},"Reason":{"type":"object"},"Node":{"type":"string"},"Role":{"type":"string"},"Detail":detail},"required":["Code","Reason"]})
    } else {
        json!({"type":"object","xml":{"name":"Fault","namespace":namespace},"properties":{"faultcode":{"type":"string"},"faultstring":{"type":"string"},"faultactor":{"type":"string"},"detail":detail},"required":["faultcode","faultstring"]})
    };
    envelope(Some(fault), headers, namespace)
}

/// WSDL 2.0 Part 2, sections 5.10.3–5.10.4: operation MEP overrides
/// binding default; an absent default is valid only for WSDL in-out.
fn response_mep(
    catalog: &Catalog,
    operation: &AbstractOperation,
    bound: &OperationBinding,
) -> Result<bool> {
    const REQUEST_RESPONSE: &str = "http://www.w3.org/2003/05/soap/mep/request-response/";
    const SOAP_RESPONSE: &str = "http://www.w3.org/2003/05/soap/mep/soap-response/";
    let pattern = catalog.node(&operation.declaration)?.attribute("pattern");
    let declared = bound.attribute(catalog, SOAP20, "mep")?.or(catalog
        .node(&bound.binding)?
        .attribute_ns(SOAP20, "mepDefault"));
    let mep = match declared {
        Some(mep) => mep,
        None if pattern == Some("http://www.w3.org/ns/wsdl/in-out") => REQUEST_RESPONSE,
        None => {
            return Err(ConversionError::new(
                "WSDL_SOAP_MEP",
                "SOAP MEP must be declared for an interface pattern other than in-out",
            ));
        }
    };
    let response = match mep {
        REQUEST_RESPONSE => false,
        SOAP_RESPONSE => true,
        _ => {
            return Err(ConversionError::new(
                "WSDL_SOAP_MEP",
                "SOAP MEP requires explicit HTTP mapping",
            ));
        }
    };
    let supported = if response {
        pattern == Some("http://www.w3.org/ns/wsdl/in-out")
    } else {
        matches!(
            pattern,
            Some(
                "http://www.w3.org/ns/wsdl/in-out"
                    | "http://www.w3.org/ns/wsdl/in-only"
                    | "http://www.w3.org/ns/wsdl/robust-in-only"
            )
        )
    };
    if !supported {
        return Err(ConversionError::new(
            "WSDL_SOAP_MEP",
            "SOAP and interface message exchange patterns are incompatible",
        ));
    }
    if response {
        for direction in ["input", "output"] {
            for at in catalog.children(&operation.declaration, WSDL20, direction)? {
                if catalog.node(&at)?.attribute("element") == Some("#any") {
                    return Err(ConversionError::new(
                        "WSDL_SOAP_MEP",
                        "SOAP-Response messages must declare an element or #none",
                    ));
                }
            }
        }
    }
    Ok(response)
}

#[derive(Clone, Default)]
struct HeaderBlocks {
    schemas: Map<String, Value>,
    required: Vec<String>,
}

fn fault_headers(
    catalog: &Catalog,
    bound: &OperationBinding,
) -> Result<BTreeMap<super::Name, HeaderBlocks>> {
    let declarations = catalog.children(&bound.binding, WSDL20, "fault")?;
    if declarations.is_empty() {
        return Ok(BTreeMap::new());
    }
    let binding = catalog.node(&bound.binding)?;
    let interface = binding.qname(binding.attribute("interface").ok_or_else(|| {
        ConversionError::new("WSDL_FAULT", "Binding faults require an interface")
    })?)?;
    let mut known = BTreeSet::new();
    for (owner, at) in catalog.interface_members(&interface, "fault")? {
        let name = catalog
            .node(&at)?
            .attribute("name")
            .ok_or_else(|| ConversionError::new("WSDL_FAULT", "Interface fault requires name"))?;
        known.insert(super::Name {
            namespace: owner.namespace,
            local: name.into(),
        });
    }
    let mut result = BTreeMap::new();
    for declaration in declarations {
        let node = catalog.node(&declaration)?;
        let name = node.qname(node.attribute("ref").ok_or_else(|| {
            ConversionError::new("WSDL_FAULT", "Binding fault requires ref QName")
        })?)?;
        if !known.contains(&name) || result.contains_key(&name) {
            return Err(ConversionError::new(
                "WSDL_FAULT",
                "Binding fault must uniquely identify an interface fault",
            ));
        }
        let mut headers = HeaderBlocks::default();
        for header in catalog.children(&declaration, SOAP20, "header")? {
            element_header(catalog, &header, &mut headers)?;
        }
        result.insert(name, headers);
    }
    Ok(result)
}

fn element_header(catalog: &Catalog, at: &NodeRef, headers: &mut HeaderBlocks) -> Result<()> {
    let node = catalog.node(at)?;
    let name = node.qname(node.attribute("element").ok_or_else(|| {
        ConversionError::new("SOAP_HEADER", "WSDL 2.0 SOAP header requires element QName")
    })?)?;
    let key = format!("{{{}}}{}", name.namespace, name.local);
    if headers.schemas.contains_key(&key) {
        return Err(ConversionError::new(
            "SOAP_HEADER",
            "SOAP message declares the same header element more than once",
        ));
    }
    let mut schema = xsd::reference(catalog, Kind::Element, &name)?;
    let must_understand = header_boolean(node.attribute("mustUnderstand"))?;
    if must_understand {
        // This flag describes SOAP processing. The coverage review preserves
        // the requirement for a SOAP-aware serializer; it is not an HTTP header.
        schema["x-soap-must-understand"] = json!(true);
    }
    if header_boolean(node.attribute("required"))? {
        headers.required.push(key.clone());
    }
    headers.schemas.insert(key, schema);
    Ok(())
}

fn header_boolean(value: Option<&str>) -> Result<bool> {
    match value.map(str::trim) {
        None | Some("false" | "0") => Ok(false),
        Some("true" | "1") => Ok(true),
        _ => Err(ConversionError::new(
            "SOAP_HEADER",
            "SOAP header flags must be XML Schema booleans",
        )),
    }
}

fn envelope(body: Option<Value>, headers: HeaderBlocks, namespace: &str) -> Value {
    let mut contents = json!({"type":"object","xml":{"name":"Body","namespace":namespace}});
    if let Some(body) = body {
        contents["properties"] = json!({"payload":body});
        contents["required"] = json!(["payload"]);
    } else {
        contents["maxProperties"] = json!(0);
    }
    let mut properties = json!({"Body":contents});
    let mut required = vec!["Body"];
    if !headers.schemas.is_empty() {
        properties["Header"] = json!({"type":"object","properties":headers.schemas,"xml":{"name":"Header","namespace":namespace}});
        if !headers.required.is_empty() {
            properties["Header"]["required"] = json!(headers.required);
            required.push("Header");
        }
    }
    json!({"type":"object","properties":properties,"required":required,"xml":{"name":"Envelope","namespace":namespace},"x-soap-envelope":true})
}

fn soap_faults(
    catalog: &Catalog,
    operation: &AbstractOperation,
    namespace: &str,
) -> Result<Vec<(Option<super::Name>, Value)>> {
    let mut faults = Vec::new();
    for fault in catalog
        .children(&operation.declaration, namespace, "fault")?
        .into_iter()
        .chain(catalog.children(&operation.declaration, namespace, "outfault")?)
    {
        let node = catalog.node(&fault)?;
        if namespace == WSDL11 {
            if let Some(payload) = message_parts(catalog, &fault, None)? {
                faults.push((None, payload));
            }
        } else if let Some(reference) = node.attribute("ref") {
            let name = node.qname(reference)?;
            let mut found = false;
            for (owner, declaration) in catalog.interface_members(&operation.interface, "fault")? {
                let node = catalog.node(&declaration)?;
                if owner.namespace == name.namespace
                    && node.attribute("name") == Some(name.local.as_str())
                {
                    found = true;
                    let element = node.attribute("element").ok_or_else(|| {
                        ConversionError::new("WSDL_FAULT", "Interface fault requires element QName")
                    })?;
                    faults.push((
                        Some(name.clone()),
                        xsd::reference(catalog, Kind::Element, &node.qname(element)?)?,
                    ));
                }
            }
            if !found {
                return Err(ConversionError::new(
                    "WSDL_FAULT",
                    "Fault reference has no interface declaration",
                ));
            }
        }
    }
    Ok(faults)
}

pub(super) fn message_parts(
    catalog: &Catalog,
    reference: &NodeRef,
    selected: Option<&str>,
) -> Result<Option<Value>> {
    let node = catalog.node(reference)?;
    let message = node.attribute("message").ok_or_else(|| {
        ConversionError::new("WSDL_MESSAGE", "Message reference requires message QName")
    })?;
    let message = catalog.declaration(Kind::Message, &node.qname(message)?)?;
    let all = catalog.children(&message.node, WSDL11, "part")?;
    let selected = selected.map(|parts| parts.split_whitespace().collect::<BTreeSet<_>>());
    let mut found = BTreeSet::new();
    let mut values = Map::new();
    for part in all {
        let node = catalog.node(&part)?;
        let name = node
            .attribute("name")
            .ok_or_else(|| ConversionError::new("WSDL_PART", "Message part requires name"))?;
        if selected
            .as_ref()
            .is_some_and(|selected| !selected.contains(name))
        {
            continue;
        }
        found.insert(name.to_owned());
        values.insert(name.into(), part_schema(catalog, node)?);
    }
    if selected
        .as_ref()
        .is_some_and(|selected| selected.iter().any(|part| !found.contains(*part)))
    {
        return Err(ConversionError::new(
            "SOAP_BODY_PARTS",
            "SOAP body selects an unknown message part",
        ));
    }
    Ok(if values.is_empty() {
        None
    } else if values.len() == 1 {
        values.into_values().next()
    } else {
        Some(json!({"type":"object","properties":values,"required":found}))
    })
}
