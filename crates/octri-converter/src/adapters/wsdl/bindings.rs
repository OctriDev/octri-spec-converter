//! Declared HTTP/SOAP endpoints. No operation name is used to invent a URL.
mod http;
mod soap;

use super::{
    AbstractOperation,
    catalog::{Catalog, Kind, NodeRef, WSDL11, WSDL20},
    part_schema, xsd,
};
use crate::adapters::http_method::HttpMethod;
use crate::adapters::xml::Name;
use crate::{
    Classification, Config, ConversionError, Result,
    source::{Sources, hash},
    trace::Trace,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

const SOAP11: &str = "http://schemas.xmlsoap.org/wsdl/soap/";
const SOAP12: &str = "http://schemas.xmlsoap.org/wsdl/soap12/";
const HTTP11: &str = "http://schemas.xmlsoap.org/wsdl/http/";
const MIME: &str = "http://schemas.xmlsoap.org/wsdl/mime/";
const HTTP20: &str = "http://www.w3.org/ns/wsdl/http";
const SOAP20: &str = "http://www.w3.org/ns/wsdl/soap";

/// Defaults do not have a physical operation declaration. Keep that distinction
/// explicit: diagnostics use the binding, but operation attributes and children
/// must never accidentally come from the binding element itself.
struct OperationBinding {
    binding: NodeRef,
    declaration: Option<NodeRef>,
}

impl OperationBinding {
    fn source(&self) -> &NodeRef {
        self.declaration.as_ref().unwrap_or(&self.binding)
    }

    fn attribute<'a>(
        &self,
        catalog: &'a Catalog,
        namespace: &str,
        local: &str,
    ) -> Result<Option<&'a str>> {
        self.declaration
            .as_ref()
            .map(|at| {
                catalog
                    .node(at)
                    .map(|node| node.attribute_ns(namespace, local))
            })
            .transpose()
            .map(Option::flatten)
    }

    fn children(&self, catalog: &Catalog, namespace: &str, local: &str) -> Result<Vec<NodeRef>> {
        match &self.declaration {
            Some(at) => catalog.children(at, namespace, local),
            None => Ok(Vec::new()),
        }
    }
}

#[derive(Debug)]
struct Endpoint {
    operation: Value,
    method: String,
    path: String,
    source: NodeRef,
}

#[derive(Debug, Default)]
pub(super) struct Native {
    pub covered: BTreeSet<String>,
    endpoints: Vec<Endpoint>,
    security_schemes: Map<String, Value>,
}

impl Native {
    pub fn merge(self, document: &mut Value, sources: &Sources, trace: &mut Trace) -> Result<()> {
        for (name, scheme) in self.security_schemes {
            document["components"]["securitySchemes"][name] = scheme;
        }
        for endpoint in self.endpoints {
            let method = HttpMethod::parse(&endpoint.method).ok_or_else(|| {
                ConversionError::new("WSDL_HTTP_METHOD", "Invalid declared HTTP method token")
            })?;
            let pointer = method.pointer(&endpoint.path);
            if document.pointer(&pointer).is_some() {
                let existing = document.pointer_mut(&pointer).ok_or_else(|| {
                    ConversionError::new("WSDL_ENDPOINT", "Endpoint disappeared during merging")
                })?;
                merge_operations(existing, endpoint.operation)?;
                trace.record(sources,&endpoint.source.uri,&format!("/xml/{}",endpoint.source.id),&[pointer],"WSDL_SHARED_HTTP_ENDPOINT",Classification::Omitted,"Several protocol operations share an HTTP method/path. Payload alternatives and action metadata are preserved; OpenAPI cannot condition each response schema on its SOAP action header.",None);
            } else {
                method.insert(document, &endpoint.path, endpoint.operation);
            }
        }
        Ok(())
    }
}

pub(super) fn convert(
    catalog: &Catalog,
    operations: &BTreeMap<NodeRef, AbstractOperation>,
    schemas: &Map<String, Value>,
    config: &Config,
    sources: &Sources,
    trace: &mut Trace,
) -> Result<Native> {
    let mut native = Native::default();
    let mut binding_work = 0;
    for ((kind, _), service) in &catalog.declarations {
        if *kind != Kind::Service {
            continue;
        }
        let service_node = catalog.node(&service.node)?;
        let namespace = service_node.name.namespace.as_str();
        let endpoint_name = if namespace == WSDL11 {
            "port"
        } else {
            "endpoint"
        };
        for endpoint in catalog.children(&service.node, namespace, endpoint_name)? {
            let node = catalog.node(&endpoint)?;
            let binding = node.attribute("binding").ok_or_else(|| {
                ConversionError::new("WSDL_BINDING", "Service endpoint requires binding QName")
            })?;
            let binding = catalog.declaration(Kind::Binding, &node.qname(binding)?)?;
            let binding_node = catalog.node(&binding.node)?;
            let interface_attribute = if namespace == WSDL11 {
                "type"
            } else {
                "interface"
            };
            let interface_node = if binding_node.attribute(interface_attribute).is_some() {
                binding_node
            } else {
                service_node
            };
            let interface = interface_node
                .attribute(interface_attribute)
                .ok_or_else(|| {
                    ConversionError::new(
                        "WSDL_BINDING",
                        "Binding requires its interface/portType QName",
                    )
                })?;
            let interface = interface_node.qname(interface)?;
            if namespace == WSDL20 {
                let service_interface = service_node.attribute("interface").ok_or_else(|| {
                    ConversionError::new(
                        "WSDL_BINDING_INTERFACE",
                        "Service requires interface QName",
                    )
                })?;
                if interface != service_node.qname(service_interface)? {
                    return Err(ConversionError::new(
                        "WSDL_BINDING_INTERFACE",
                        "Endpoint binding interface must equal its service interface",
                    ));
                }
                catalog.declaration(Kind::Interface, &interface)?;
                if binding_node.attribute("interface").is_none()
                    && (!catalog
                        .children(&binding.node, WSDL20, "operation")?
                        .is_empty()
                        || !catalog.children(&binding.node, WSDL20, "fault")?.is_empty())
                {
                    return Err(ConversionError::new(
                        "WSDL_REUSABLE_BINDING",
                        "A reusable binding without an interface cannot declare operation or fault bindings",
                    ));
                }
            }
            let (protocol, address) = if namespace == WSDL11 {
                let mut addresses = Vec::new();
                for protocol in [SOAP11, SOAP12, HTTP11] {
                    for address in catalog.children(&endpoint, protocol, "address")? {
                        let location =
                            catalog
                                .node(&address)?
                                .attribute("location")
                                .ok_or_else(|| {
                                    ConversionError::new(
                                        "WSDL_ENDPOINT",
                                        "Binding address requires location",
                                    )
                                })?;
                        addresses.push((
                            protocol,
                            absolute_endpoint(catalog, &address, location, sources)?,
                        ));
                    }
                }
                if addresses.len() != 1 {
                    continue;
                }
                addresses.remove(0)
            } else {
                let protocol = match binding_node.attribute("type") {
                    Some("http://www.w3.org/ns/wsdl/http") => HTTP20,
                    Some("http://www.w3.org/ns/wsdl/soap") => SOAP20,
                    _ => continue,
                };
                let address = node.attribute("address").ok_or_else(|| {
                    ConversionError::new("WSDL_ENDPOINT", "Endpoint requires address")
                })?;
                (
                    protocol,
                    absolute_endpoint(catalog, &endpoint, address, sources)?,
                )
            };
            if !matches!(address.scheme(), "http" | "https") {
                continue;
            }
            let selected = select_operations(
                catalog,
                &binding.node,
                &interface,
                operations,
                sources,
                trace,
                &mut binding_work,
            )?;
            for (operation, bound) in selected {
                let source = bound.source();
                let bound_node = catalog.node(source)?;
                if config.http_mappings.contains_key(&operation.key) {
                    trace.record(sources,&source.uri,&bound_node.location.address.pointer,&[],"WSDL_TRANSPORT_REMAPPED",Classification::Omitted,"Caller HTTP mapping replaces declared WSDL transport. Original endpoint, SOAP envelope, action and fault behavior remain in source; review replacement contract.",Some(&json!(operation.key)));
                    continue;
                }
                let emitted = if matches!(protocol, SOAP11 | SOAP12 | SOAP20) {
                    soap::emit(
                        catalog, operation, &bound, protocol, &address, schemas, sources, trace,
                    )?
                } else {
                    http::emit(
                        catalog,
                        operation,
                        &bound,
                        if protocol == HTTP20 {
                            http::Transport::Http20
                        } else {
                            http::Transport::Http11
                        },
                        &address,
                        schemas,
                        sources,
                        trace,
                    )?
                };
                if let Some(mut emitted_endpoint) = emitted {
                    if namespace == WSDL20 {
                        if let Some(scheme) = node.attribute_ns(HTTP20, "authenticationScheme") {
                            let scheme = scheme.to_ascii_lowercase();
                            if matches!(scheme.as_str(), "basic" | "digest") {
                                let name = format!(
                                    "WsdlHttp_{}",
                                    &hash(format!("{scheme} {address}").as_bytes())[..16]
                                );
                                native
                                    .security_schemes
                                    .insert(name.clone(), json!({"type":"http","scheme":scheme}));
                                emitted_endpoint.operation["security"] = json!([{name:[]}]);
                                if let Some(realm) =
                                    node.attribute_ns(HTTP20, "authenticationRealm")
                                {
                                    emitted_endpoint.operation["x-http-authentication-realm"] =
                                        json!(realm);
                                }
                            } else {
                                record(
                                    catalog,
                                    &endpoint,
                                    sources,
                                    trace,
                                    "WSDL_HTTP_AUTH_MAPPING",
                                    Classification::Blocked,
                                    "Declared HTTP authentication scheme needs an explicit security mapping",
                                );
                            }
                        } else {
                            record(
                                catalog,
                                &endpoint,
                                sources,
                                trace,
                                "WSDL_HTTP_AUTH_UNDECLARED",
                                Classification::Inferred,
                                "WSDL endpoint does not establish HTTP authentication. Review absent security or supply explicit mapping security requirements.",
                            );
                        }
                    } else {
                        record(
                            catalog,
                            &endpoint,
                            sources,
                            trace,
                            "WSDL_HTTP_AUTH_UNDECLARED",
                            Classification::Inferred,
                            "WSDL 1.1 binding does not establish HTTP authentication. Review absent security or supply explicit mapping security requirements.",
                        );
                    }
                    native.covered.insert(operation.key.clone());
                    native.endpoints.push(emitted_endpoint);
                }
            }
        }
    }
    Ok(native)
}

/// Explicit declarations override defaults for the same physical interface
/// operation. WSDL 1.1 has no corresponding implicit operation binding rule.
#[allow(clippy::too_many_arguments)]
fn select_operations<'a>(
    catalog: &Catalog,
    binding: &NodeRef,
    interface: &Name,
    operations: &'a BTreeMap<NodeRef, AbstractOperation>,
    sources: &Sources,
    trace: &mut Trace,
    work: &mut usize,
) -> Result<Vec<(&'a AbstractOperation, OperationBinding)>> {
    let modern = catalog.node(binding)?.name.namespace == WSDL20;
    let namespace = if modern { WSDL20 } else { WSDL11 };
    let declarations = catalog.children(binding, namespace, "operation")?;
    *work = work
        .saturating_add(operations.len())
        .saturating_add(declarations.len());
    if *work > sources.limits.max_nodes {
        return Err(ConversionError::new(
            "WSDL_BINDING_LIMIT",
            "Binding expansion exceeds node work limit",
        ));
    }
    let mut members: BTreeMap<Name, Vec<&AbstractOperation>> = BTreeMap::new();
    let mut available = Vec::new();
    for operation in operations.values().filter(|operation| {
        if modern {
            catalog.interface_contains(interface, &operation.interface)
        } else {
            operation.interface == *interface
        }
    }) {
        let local = catalog
            .node(&operation.declaration)?
            .attribute("name")
            .ok_or_else(|| {
                ConversionError::new("WSDL_OPERATION", "Interface operation requires name")
            })?;
        members
            .entry(Name {
                namespace: operation.interface.namespace.clone(),
                local: local.into(),
            })
            .or_default()
            .push(operation);
        available.push(operation);
    }
    let mut selected = Vec::new();
    if modern {
        // Fault schemas and headers are projected for each bound operation.
        // Count their expansion too, rather than bounding endpoint count alone.
        let mut fault_work = catalog.interface_members(interface, "fault")?.len();
        for fault in catalog.children(binding, WSDL20, "fault")? {
            fault_work = fault_work
                .saturating_add(1)
                .saturating_add(catalog.children(&fault, SOAP20, "header")?.len())
                .saturating_add(catalog.children(&fault, HTTP20, "header")?.len());
        }
        *work = work.saturating_add(available.len().saturating_mul(fault_work));
        if *work > sources.limits.max_nodes {
            return Err(ConversionError::new(
                "WSDL_BINDING_LIMIT",
                "Fault and header expansion exceeds node work limit",
            ));
        }
    }
    let mut seen = BTreeSet::new();
    for at in declarations {
        let node = catalog.node(&at)?;
        let name = if modern {
            node.qname(node.attribute("ref").ok_or_else(|| {
                ConversionError::new("WSDL_OPERATION", "Binding operation requires ref QName")
            })?)?
        } else {
            Name {
                namespace: interface.namespace.clone(),
                local: node
                    .attribute("name")
                    .ok_or_else(|| {
                        ConversionError::new("WSDL_OPERATION", "Binding operation requires name")
                    })?
                    .into(),
            }
        };
        let candidates = members.get(&name).map(Vec::as_slice).unwrap_or_default();
        if candidates.len() != 1 {
            trace.record(sources,&at.uri,&node.location.address.pointer,&[],"WSDL_OVERLOADED_BINDING",Classification::Blocked,"Binding operation must identify one interface operation. Supply explicit mappings for overloaded operations.",None);
            continue;
        }
        let operation = candidates[0];
        if !seen.insert(&operation.declaration) {
            return Err(ConversionError::new(
                "WSDL_DUPLICATE_BINDING_OPERATION",
                "Binding declares the same interface operation more than once",
            ));
        }
        selected.push((
            operation,
            OperationBinding {
                binding: binding.clone(),
                declaration: Some(at),
            },
        ));
    }
    if modern {
        for operation in available {
            if seen.insert(&operation.declaration) {
                selected.push((
                    operation,
                    OperationBinding {
                        binding: binding.clone(),
                        declaration: None,
                    },
                ));
            }
        }
    }
    Ok(selected)
}

fn absolute_endpoint(
    catalog: &Catalog,
    node: &NodeRef,
    value: &str,
    sources: &Sources,
) -> Result<url::Url> {
    let uri = if let Ok(url) = url::Url::parse(value) {
        url.to_string()
    } else {
        catalog.relative(node, value, sources)?
    };
    let url = url::Url::parse(&uri)
        .map_err(|error| ConversionError::new("WSDL_ENDPOINT", error.to_string()))?;
    if matches!(url.scheme(), "http" | "https")
        && (url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some())
    {
        return Err(ConversionError::new(
            "WSDL_ENDPOINT",
            "Native endpoint requires an HTTP(S) URL without query or fragment; provide explicit HTTP mapping for another transport",
        ));
    }
    Ok(url)
}

fn base_operation(operation: &AbstractOperation, address: &url::Url, method: &str) -> Value {
    let server = address.origin().ascii_serialization();
    json!({"operationId":format!("wsdl_{}",&hash(format!("{} {method} {address}",operation.key).as_bytes())[..20]),"summary":operation.key,"servers":[{"url":server}],"responses":{},"x-wsdl-operation":operation.key})
}

fn record(
    catalog: &Catalog,
    at: &NodeRef,
    sources: &Sources,
    trace: &mut Trace,
    code: &str,
    classification: Classification,
    message: impl Into<String>,
) {
    if let Ok(node) = catalog.node(at) {
        trace.record(
            sources,
            &at.uri,
            &node.location.address.pointer,
            &[],
            code,
            classification,
            message,
            None,
        );
    }
}

fn merge_operations(existing: &mut Value, incoming: Value) -> Result<()> {
    // An OpenAPI operation represents alternatives at a shared endpoint. Keep
    // each security requirement object intact: keys within one object are AND,
    // whereas objects within the array are OR. Missing security inherits the
    // generated document's empty root requirement and remains reviewable.
    let mut security = existing["security"]
        .as_array()
        .cloned()
        .filter(|requirements| !requirements.is_empty())
        .unwrap_or_else(|| vec![json!({})]);
    for requirement in incoming["security"]
        .as_array()
        .cloned()
        .filter(|requirements| !requirements.is_empty())
        .unwrap_or_else(|| vec![json!({})])
    {
        if !security.contains(&requirement) {
            security.push(requirement);
        }
    }
    let body_required = existing.pointer("/requestBody/required") == Some(&Value::Bool(true))
        && incoming.pointer("/requestBody/required") == Some(&Value::Bool(true));
    let previous_parameters = existing["parameters"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let previous = if existing.get("x-wsdl-variants").is_none() {
        Some(existing.clone())
    } else {
        None
    };
    let existing_map = existing.as_object_mut().ok_or_else(|| {
        ConversionError::new("WSDL_ENDPOINT", "Generated endpoint must be an object")
    })?;
    existing_map
        .entry("x-wsdl-variants")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| ConversionError::new("WSDL_ENDPOINT", "Variant registry must be an array"))?
        .extend(
            previous
                .into_iter()
                .chain(std::iter::once(incoming.clone())),
        );
    existing["security"] = json!(security);
    for server in incoming["servers"].as_array().into_iter().flatten() {
        if let Some(servers) = existing["servers"].as_array_mut()
            && !servers.contains(server)
        {
            servers.push(server.clone());
        }
    }
    for (media, payload) in incoming
        .pointer("/requestBody/content")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if let Some(schema) = existing.pointer_mut(&format!(
            "/requestBody/content/{}/schema",
            media.replace('~', "~0").replace('/', "~1")
        )) {
            *schema = json!({"anyOf":[schema.clone(),payload["schema"]]});
        } else {
            existing["requestBody"]["content"][media] = payload.clone();
        }
    }
    for (status, response) in incoming["responses"].as_object().into_iter().flatten() {
        if existing["responses"].get(status).is_none() {
            existing["responses"][status] = response.clone();
        } else {
            for (media, payload) in response["content"].as_object().into_iter().flatten() {
                if let Some(schema) =
                    existing["responses"][status]["content"][media].get_mut("schema")
                {
                    *schema = json!({"anyOf":[schema.clone(),payload["schema"]]});
                } else {
                    existing["responses"][status]["content"][media] = payload.clone();
                }
            }
        }
    }
    if existing.get("requestBody").is_some() {
        existing["requestBody"]["required"] = json!(body_required);
    }
    if let Some(parameters) = existing.get_mut("parameters").and_then(Value::as_array_mut) {
        for parameter in parameters {
            // A parameter can be required only when every variant requires it.
            // Path parameters remain required by the OpenAPI contract.
            if parameter["in"] != "path" {
                let required = parameter["required"] == true
                    && incoming["parameters"].as_array().is_some_and(|parameters| {
                        parameters.iter().any(|other| {
                            parameter["name"] == other["name"]
                                && parameter["in"] == other["in"]
                                && other["required"] == true
                        })
                    });
                parameter["required"] = json!(required);
            }
        }
    }
    for incoming_parameter in incoming["parameters"].as_array().into_iter().flatten() {
        let mut merged = false;
        if let Some(parameters) = existing.get_mut("parameters").and_then(Value::as_array_mut) {
            for parameter in parameters.iter_mut() {
                if parameter["name"] == incoming_parameter["name"]
                    && parameter["in"] == incoming_parameter["in"]
                {
                    parameter["schema"] =
                        json!({"anyOf":[parameter["schema"],incoming_parameter["schema"]]});
                    merged = true;
                    break;
                }
            }
        }
        if !merged {
            if existing.get("parameters").is_none() {
                existing["parameters"] = json!([]);
            }
            if let Some(parameters) = existing["parameters"].as_array_mut() {
                let mut parameter = incoming_parameter.clone();
                if parameter["in"] != "path" {
                    parameter["required"] = json!(
                        incoming_parameter["required"] == true
                            && previous_parameters.iter().any(|previous| {
                                previous["name"] == incoming_parameter["name"]
                                    && previous["in"] == incoming_parameter["in"]
                                    && previous["required"] == true
                            })
                    );
                }
                parameters.push(parameter);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_endpoint_keeps_auth_alternatives_and_conditional_requirements() -> Result<()> {
        let mut first = json!({"security":[{"basic":[],"key":[]}],"parameters":[
            {"name":"A","in":"header","required":true,"schema":{"const":"one"}},
            {"name":"Shared","in":"header","required":true,"schema":{"type":"string"}}
        ],"requestBody":{"required":true,"content":{"text/xml":{"schema":{"type":"string"}}}},"responses":{"200":{"description":"first"}}});
        let second = json!({"security":[{"digest":[]}],"parameters":[
            {"name":"B","in":"header","required":true,"schema":{"const":"two"}},
            {"name":"Shared","in":"header","required":true,"schema":{"type":"string"}}
        ],"responses":{"200":{"description":"second"}}});
        merge_operations(&mut first, second)?;
        assert_eq!(
            first["security"],
            json!([{"basic":[],"key":[]},{"digest":[]}])
        );
        assert_eq!(first["requestBody"]["required"], false);
        assert_eq!(first["parameters"][0]["required"], false);
        assert_eq!(first["parameters"][1]["required"], true);
        assert_eq!(first["parameters"][2]["required"], false);
        merge_operations(
            &mut first,
            json!({"responses":{"202":{"description":"third"}}}),
        )?;
        assert_eq!(
            first["security"],
            json!([{"basic":[],"key":[]},{"digest":[]},{}])
        );
        assert_eq!(first["parameters"][1]["required"], false);
        Ok(())
    }
}
