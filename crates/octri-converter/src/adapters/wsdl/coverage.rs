//! Account for source constraints that OpenAPI consumers cannot enforce.
use super::catalog::{Catalog, WSDL11, WSDL20, XSD};
use crate::{
    Classification,
    source::{Sources, child},
    trace::Trace,
};

const SOAP11: &str = "http://schemas.xmlsoap.org/wsdl/soap/";
const SOAP12: &str = "http://schemas.xmlsoap.org/wsdl/soap12/";
const SOAP20: &str = "http://www.w3.org/ns/wsdl/soap";
const HTTP11: &str = "http://schemas.xmlsoap.org/wsdl/http/";
const HTTP20: &str = "http://www.w3.org/ns/wsdl/http";
const MIME: &str = "http://schemas.xmlsoap.org/wsdl/mime/";

pub(super) fn review(catalog: &Catalog, sources: &Sources, trace: &mut Trace) {
    for document in catalog.documents.values() {
        for node in &document.nodes {
            // Mark foreign markup once at its outer boundary. XHTML inside
            // documentation remains inert prose, not a protocol extension.
            let parent = node.parent.and_then(|id| document.nodes.get(id));
            let mut ancestor = node.parent;
            let mut documentation = false;
            while let Some(id) = ancestor {
                let node = &document.nodes[id];
                if matches!(node.name.namespace.as_str(), WSDL11 | WSDL20 | XSD)
                    && matches!(node.name.local.as_str(), "documentation" | "annotation")
                {
                    documentation = true;
                    break;
                }
                ancestor = node.parent;
            }
            if documentation {
                continue;
            }
            let namespace = node.name.namespace.as_str();
            if !matches!(
                namespace,
                WSDL11 | WSDL20 | XSD | SOAP11 | SOAP12 | SOAP20 | HTTP11 | HTTP20 | MIME
            ) && parent.is_none_or(|parent| parent.name.namespace != node.name.namespace)
            {
                let required = node
                    .attribute_ns(WSDL11, "required")
                    .or_else(|| node.attribute_ns(WSDL20, "required"))
                    .is_some_and(|value| matches!(value, "true" | "1"));
                trace.record(sources,&node.location.address.uri,&node.location.address.pointer,&[],"WSDL_PROTOCOL_EXTENSION",if required {Classification::Blocked}else{Classification::Omitted},format!("Protocol extension {{{namespace}}}{} remains in source. Its transport, security or policy behavior needs explicit review{}.",node.name.local,if required {" and an authored representation"}else{""}),None);
            }
            if namespace == MIME && matches!(node.name.local.as_str(), "multipartRelated" | "part")
            {
                trace.record(sources,&node.location.address.uri,&node.location.address.pointer,&[],"WSDL_MIME_MULTIPART",Classification::Omitted,"MIME multipart framing and attachment relationships remain in source. OpenAPI payload projection needs a compatible serializer before publishing.",None);
            }
            if namespace == SOAP20 && matches!(node.name.local.as_str(), "module" | "header") {
                trace.record(sources,&node.location.address.uri,&node.location.address.pointer,&[],"WSDL_SOAP_MODULE",Classification::Omitted,"SOAP module/header processing requirements remain in source; configure SOAP-aware client behavior.",None);
            }
            for (attribute, value) in &node.attributes {
                let code = if attribute.namespace == XSD
                    || attribute.namespace.is_empty() && namespace == XSD
                {
                    match attribute.local.as_str() {
                        "substitutionGroup"
                        | "block"
                        | "blockDefault"
                        | "final"
                        | "finalDefault"
                        | "defaultAttributes"
                        | "xpathDefaultNamespace" => Some("XSD_DERIVATION_CONSTRAINT"),
                        _ => None,
                    }
                } else if attribute.namespace == HTTP20 {
                    match attribute.local.as_str() {
                        "queryParameterSeparator" | "queryParameterSeparatorDefault"
                            if value != "&" =>
                        {
                            Some("WSDL_QUERY_SEPARATOR")
                        }
                        "ignoreUncited" if matches!(value.as_str(), "true" | "1") => {
                            Some("WSDL_UNCITED_INPUT")
                        }
                        "contentEncoding" | "contentEncodingDefault" if !value.is_empty() => {
                            Some("WSDL_CONTENT_ENCODING")
                        }
                        "cookies" => Some("WSDL_COOKIE_BEHAVIOR"),
                        _ => None,
                    }
                } else if !attribute.namespace.is_empty()
                    && !matches!(
                        attribute.namespace.as_str(),
                        WSDL11
                            | WSDL20
                            | SOAP11
                            | SOAP12
                            | SOAP20
                            | HTTP11
                            | MIME
                            | "http://www.w3.org/XML/1998/namespace"
                            | "http://www.w3.org/ns/wsdl-extensions"
                    )
                {
                    Some("WSDL_ATTRIBUTE_EXTENSION")
                } else {
                    None
                };
                if let Some(code) = code {
                    trace.record(sources,&node.location.address.uri,&child(&node.location.address.pointer,&format!("@{{{}}}{}",attribute.namespace,attribute.local)),&[],code,Classification::Omitted,format!("Source attribute {{{}}}{}={value:?} remains in original source; emitted OpenAPI does not enforce this behavior.",attribute.namespace,attribute.local),None);
                }
            }
        }
    }
}
