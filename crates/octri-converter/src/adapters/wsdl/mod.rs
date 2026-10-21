//! WSDL declarations, XML Schema projection and explicit HTTP remapping.
mod bindings;
mod catalog;
mod coverage;
mod interfaces;
mod xsd;

use super::mapped;
use crate::{Classification, Config, ConversionError, Result, source::Sources, trace::Trace};
use catalog::{Catalog, Kind, NodeRef, WSDL11, WSDL20};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Project a complete standalone XSD using the WSDL schema implementation.
/// Component names include source identity because different RAML declarations
/// may use the same XML QName with different validation contracts.
pub(super) struct ProjectedSchema {
    pub root: Option<Value>,
    pub elements: BTreeMap<String, Value>,
    pub complex_types: BTreeMap<String, Value>,
    pub definitions: Map<String, Value>,
}

pub(super) fn project_schema_text(
    text: &str,
    physical: &octri_parser::Address,
    sources: &mut Sources,
    trace: &mut Trace,
) -> Result<ProjectedSchema> {
    let catalog = Catalog::schema_text(text, physical, sources)?;
    coverage::review(&catalog, sources, trace);
    let mut projection = xsd::Projection::new(&catalog, sources, trace);
    let mut roots = Vec::new();
    let mut elements = BTreeMap::new();
    let mut complex_types = BTreeMap::new();
    let namespace = catalog
        .schemas
        .first()
        .map(|schema| schema.namespace.as_str())
        .unwrap_or("");
    for (kind, name) in catalog.declarations.keys() {
        if *kind == Kind::Element {
            roots.push(projection.reference(*kind, name)?);
            if name.namespace == namespace {
                elements.insert(name.local.clone(), projection.reference(*kind, name)?);
            }
        } else if *kind == Kind::Type
            && name.namespace == namespace
            && catalog
                .node(&catalog.declaration(*kind, name)?.node)?
                .is(catalog::XSD, "complexType")
        {
            complex_types.insert(name.local.clone(), projection.reference(*kind, name)?);
        }
    }
    let mut schema = if roots.is_empty() {
        None
    } else if roots.len() == 1 {
        Some(roots.remove(0))
    } else {
        Some(json!({"anyOf":roots}))
    };
    let definitions = projection.components()?;
    let scope =
        &crate::source::hash(format!("{}#{}:{text}", physical.uri, physical.pointer).as_bytes())
            [..16];
    let names: BTreeMap<_, _> = definitions
        .keys()
        .map(|name| (name.clone(), format!("RamlXsd_{scope}_{name}")))
        .collect();
    let rewrite = |value: &mut Value| {
        crate::schema::walk(value, "", &mut |schema, _| {
            if let Some(reference) = schema.get("$ref").and_then(Value::as_str)
                && let Some(name) = reference.strip_prefix("#/components/schemas/")
                && let Some(name) = names.get(name)
            {
                schema["$ref"] = json!(format!(
                    "#{}",
                    crate::source::child("/components/schemas", name)
                ));
            }
        });
    };
    if let Some(schema) = &mut schema {
        rewrite(schema);
    }
    for value in elements.values_mut().chain(complex_types.values_mut()) {
        rewrite(value);
    }
    let definitions = definitions
        .into_iter()
        .map(|(name, mut schema)| {
            rewrite(&mut schema);
            (names[&name].clone(), schema)
        })
        .collect();
    trace.record(sources, &physical.uri, &physical.pointer, &[], "RAML_XSD_PROJECTION", Classification::Transformed,
        "XML Schema declarations projected with their imports, XML names and constraints using the shared WSDL/XSD engine", None);
    trace.record(sources, &physical.uri, &physical.pointer, &[], "RAML_XML_SERIALIZATION", Classification::Inferred,
        "Review the receiving OpenAPI consumer's XML serializer. JSON Schema validates the projected value model; XML names, ordering and lexical rules require an XML-aware consumer.", None);
    Ok(ProjectedSchema {
        root: schema,
        elements,
        complex_types,
        definitions,
    })
}

struct AbstractOperation {
    key: String,
    interface: super::xml::Name,
    declaration: NodeRef,
    input: Option<Value>,
    output: Option<Value>,
}

pub(crate) fn convert(sources: &mut Sources, config: &Config, trace: &mut Trace) -> Result<Value> {
    let catalog = Catalog::load(sources)?;
    coverage::review(&catalog, sources, trace);
    let root = catalog.root(&sources.root)?;
    let root_node = catalog.node(&root)?;
    let title = root_node
        .attribute("name")
        .unwrap_or("Converted WSDL contract")
        .to_owned();
    let mut projection = xsd::Projection::new(&catalog, sources, trace);
    let mut operations = Vec::new();
    let mut abstract_operations = BTreeMap::new();
    for ((kind, name), declaration) in &catalog.declarations {
        let namespace = match kind {
            Kind::PortType => WSDL11,
            Kind::Interface => WSDL20,
            _ => continue,
        };
        let declarations = catalog.children(&declaration.node, namespace, "operation")?;
        for (ordinal, operation) in declarations.iter().enumerate() {
            let node = catalog.node(operation)?;
            let operation_name = node.attribute("name").ok_or_else(|| {
                ConversionError::new("WSDL_OPERATION_NAME", "Declared operation requires a name")
            })?;
            let duplicates = declarations
                .iter()
                .filter(|other| {
                    catalog
                        .node(other)
                        .is_ok_and(|node| node.attribute("name") == Some(operation_name))
                })
                .count()
                > 1;
            let mut key = format!("{{{}}}{}.{}", name.namespace, name.local, operation_name);
            if duplicates {
                key.push_str(&format!("#{ordinal}"));
            }
            let inputs = catalog.children(operation, namespace, "input")?;
            let outputs = catalog.children(operation, namespace, "output")?;
            if inputs.len() > 1 || outputs.len() > 1 {
                return Err(ConversionError::new(
                    "WSDL_MESSAGE_PATTERN",
                    "Operation has multiple input/output declarations",
                ));
            }
            let input = inputs
                .first()
                .map(|input| payload(&catalog, input, namespace))
                .transpose()?
                .flatten();
            let output = outputs
                .first()
                .map(|output| payload(&catalog, output, namespace))
                .transpose()?
                .flatten();
            let description = catalog
                .children(operation, namespace, "documentation")?
                .iter()
                .map(|at| catalog.node(at).map(|node| node.text.trim().to_owned()))
                .collect::<Result<Vec<_>>>()?
                .join("\n\n");
            operations.push(mapped::Operation {
                key: key.clone(),
                uri: operation.uri.to_string(),
                pointer: node.location.address.pointer.clone(),
                description: if description.is_empty() {
                    None
                } else {
                    Some(description)
                },
                input: input.clone(),
                output: output.clone(),
                request_streaming: false,
                response_streaming: false,
                payload_location_required: false,
            });
            abstract_operations.insert(
                operation.clone(),
                AbstractOperation {
                    key,
                    interface: name.clone(),
                    declaration: operation.clone(),
                    input,
                    output,
                },
            );
        }
    }
    let schemas = projection.components()?;
    drop(projection);
    mapped::describe(&operations, sources, trace);
    let native = bindings::convert(
        &catalog,
        &abstract_operations,
        &schemas,
        config,
        sources,
        trace,
    )?;
    operations.retain(|operation| !native.covered.contains(&operation.key));
    for ((kind, _), declaration) in &catalog.declarations {
        if !matches!(kind, Kind::PortType | Kind::Interface) {
            continue;
        }
        let namespace = if *kind == Kind::PortType {
            WSDL11
        } else {
            WSDL20
        };
        let node = catalog.node(&declaration.node)?;
        if node.attribute("extends").is_some() {
            trace.record(
                sources,
                &node.location.address.uri,
                &node.location.address.pointer,
                &[],
                "WSDL_INTERFACE_INHERITANCE",
                Classification::Transformed,
                "Inherited operations and faults retain their declaring namespace and physical source; shared ancestors are included once",
                None,
            );
        }
        for operation in catalog.children(&declaration.node, namespace, "operation")? {
            if abstract_operations
                .get(&operation)
                .is_some_and(|operation| native.covered.contains(&operation.key))
            {
                continue;
            }
            for fault in catalog
                .children(&operation, namespace, "fault")?
                .into_iter()
                .chain(catalog.children(&operation, namespace, "infault")?)
                .chain(catalog.children(&operation, namespace, "outfault")?)
            {
                let node = catalog.node(&fault)?;
                trace.record(sources,&node.location.address.uri,&node.location.address.pointer,&[],"WSDL_FAULT_HTTP_MAPPING",Classification::Omitted,"Fault declarations remain in source; caller HTTP response mapping does not establish individual SOAP/HTTP fault statuses",None);
            }
        }
    }
    if operations.is_empty() && native.covered.is_empty() {
        return Err(ConversionError::new(
            "WSDL_OPERATIONS",
            "No declared interface operations found",
        ));
    }
    let mut document = mapped::emit(
        &title,
        None,
        Value::Object(schemas),
        operations,
        sources,
        config,
        trace,
    )?;
    native.merge(&mut document, sources, trace)?;
    Ok(document)
}

fn payload(catalog: &Catalog, at: &NodeRef, namespace: &str) -> Result<Option<Value>> {
    let node = catalog.node(at)?;
    if namespace == WSDL20 {
        let element = node.attribute("element").ok_or_else(|| {
            ConversionError::new(
                "WSDL_ELEMENT",
                "WSDL 2.0 message reference requires element",
            )
        })?;
        return match element {
            "#none" => Ok(None),
            "#any" => Ok(Some(json!({}))),
            _ => Ok(Some(xsd::reference(
                catalog,
                Kind::Element,
                &node.qname(element)?,
            )?)),
        };
    }
    let message = node.attribute("message").ok_or_else(|| {
        ConversionError::new("WSDL_MESSAGE", "WSDL 1.1 input/output requires message")
    })?;
    let message = catalog.declaration(Kind::Message, &node.qname(message)?)?;
    let parts = catalog.children(&message.node, WSDL11, "part")?;
    let mut properties = Map::new();
    let mut required = Vec::new();
    for part in &parts {
        let node = catalog.node(part)?;
        let name = node
            .attribute("name")
            .ok_or_else(|| ConversionError::new("WSDL_PART", "Message part requires name"))?;
        let schema = part_schema(catalog, node)?;
        if parts.len() == 1 && node.attribute("element").is_some() {
            return Ok(Some(schema));
        }
        if properties.insert(name.into(), schema).is_some() {
            return Err(ConversionError::new(
                "WSDL_PART",
                "Message part name is duplicated",
            ));
        }
        required.push(json!(name));
    }
    Ok(if properties.is_empty() {
        None
    } else {
        Some(json!({"type":"object","properties":properties,"required":required}))
    })
}

fn part_schema(catalog: &Catalog, node: &super::xml::Node) -> Result<Value> {
    match (node.attribute("element"), node.attribute("type")) {
        (Some(element), None) => xsd::reference(catalog, Kind::Element, &node.qname(element)?),
        (None, Some(kind)) => xsd::reference(catalog, Kind::Type, &node.qname(kind)?),
        _ => Err(ConversionError::new(
            "WSDL_PART_TYPE",
            "Message part requires exactly one element or type QName",
        )),
    }
}
