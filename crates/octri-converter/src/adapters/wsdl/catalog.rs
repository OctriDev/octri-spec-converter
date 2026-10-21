//! Physical WSDL/XSD documents and namespace-qualified declaration identity.
use super::super::xml::{self, Document, Name, Node};
use crate::{ConversionError, Result, source::Sources};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub(super) const WSDL11: &str = "http://schemas.xmlsoap.org/wsdl/";
pub(super) const WSDL20: &str = "http://www.w3.org/ns/wsdl";
pub(super) const XSD: &str = "http://www.w3.org/2001/XMLSchema";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct NodeRef {
    pub uri: Arc<str>,
    pub id: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Kind {
    Message,
    PortType,
    Interface,
    Binding,
    Service,
    Type,
    Element,
    Attribute,
    Group,
    AttributeGroup,
}

#[derive(Debug, Clone)]
pub(super) struct Schema {
    pub namespace: String,
    pub chameleon: bool,
    pub element_qualified: bool,
    pub attribute_qualified: bool,
}

#[derive(Debug, Clone)]
pub(super) struct Declaration {
    pub node: NodeRef,
    pub schema: Option<usize>,
}

#[derive(Debug)]
pub(super) struct Catalog {
    pub documents: BTreeMap<String, Document>,
    pub declarations: BTreeMap<(Kind, Name), Declaration>,
    pub schemas: Vec<Schema>,
    interfaces: BTreeMap<Name, BTreeSet<Name>>,
    schema_instances: BTreeSet<(NodeRef, String)>,
}

impl Catalog {
    fn new() -> Self {
        Self {
            documents: BTreeMap::new(),
            declarations: BTreeMap::new(),
            schemas: Vec::new(),
            interfaces: BTreeMap::new(),
            schema_instances: BTreeSet::new(),
        }
    }

    pub fn load(sources: &mut Sources) -> Result<Self> {
        let mut catalog = Self::new();
        let uri = sources.root.clone();
        catalog.load_document(&uri, sources, 0)?;
        let root = catalog.root(&uri)?;
        let root = catalog.node(&root)?;
        if !root.is(WSDL11, "definitions") && !root.is(WSDL20, "description") {
            return Err(ConversionError::new(
                "WSDL_ROOT",
                "Expected namespace-qualified WSDL 1.1 definitions or WSDL 2.0 description",
            ));
        }
        catalog.interfaces = super::interfaces::resolve(&catalog, &sources.limits)?;
        Ok(catalog)
    }

    /// Use the same QName/import implementation for standalone RAML XSD bodies.
    pub fn schema_text(
        text: &str,
        physical: &octri_parser::Address,
        sources: &mut Sources,
    ) -> Result<Self> {
        let mut catalog = Self::new();
        if physical.pointer.is_empty()
            && sources
                .files
                .get(&physical.uri)
                .is_some_and(|original| original == text)
        {
            catalog.load_document(&physical.uri, sources, 0)?;
            let root = catalog.root(&physical.uri)?;
            if !catalog.node(&root)?.is(XSD, "schema") {
                return Err(ConversionError::new(
                    "XSD_ROOT",
                    "Expected namespace-qualified XML Schema document",
                ));
            }
            catalog.load_schema(root, None, sources, 0)?;
            return Ok(catalog);
        }
        let uri = sources.relative(
            &physical.uri,
            &format!(
                ".octri-xsd-{}.xsd",
                &crate::source::hash(
                    format!("{}#{}:{text}", physical.uri, physical.pointer).as_bytes()
                )[..24]
            ),
        )?;
        if sources.files.contains_key(&uri) {
            return Err(ConversionError::new(
                "XSD_DERIVED_URI",
                "Embedded schema identity conflicts with supplied source",
            ));
        }
        let document = xml::parse_embedded(sources, &uri, text, physical)?;
        catalog.documents.insert(uri.clone(), document);
        let root = catalog.root(&uri)?;
        if !catalog.node(&root)?.is(XSD, "schema") {
            return Err(ConversionError::new(
                "XSD_ROOT",
                "Expected namespace-qualified XML Schema document",
            ));
        }
        catalog.load_schema(root.clone(), None, sources, 0)?;
        Ok(catalog)
    }

    pub fn root(&self, uri: &str) -> Result<NodeRef> {
        let document = self.documents.get(uri).ok_or_else(|| {
            ConversionError::new("XML_SOURCE", format!("Document is not loaded: {uri}"))
        })?;
        Ok(NodeRef {
            uri: uri.into(),
            id: document.root,
        })
    }

    pub fn node(&self, node: &NodeRef) -> Result<&Node> {
        self.documents
            .get(node.uri.as_ref())
            .and_then(|document| document.nodes.get(node.id))
            .ok_or_else(|| ConversionError::new("XML_SOURCE", "Physical XML node is missing"))
    }

    pub fn children(&self, node: &NodeRef, namespace: &str, local: &str) -> Result<Vec<NodeRef>> {
        self.node(node)?;
        let document = self
            .documents
            .get(node.uri.as_ref())
            .ok_or_else(|| ConversionError::new("XML_SOURCE", "Document is missing"))?;
        Ok(document
            .children(node.id, namespace, local)
            .map(|id| NodeRef {
                uri: node.uri.clone(),
                id,
            })
            .collect())
    }

    pub fn declaration(&self, kind: Kind, name: &Name) -> Result<&Declaration> {
        self.declarations.get(&(kind, name.clone())).ok_or_else(|| {
            ConversionError::new(
                "WSDL_REFERENCE",
                format!("Missing {kind:?} {{{}}}{}", name.namespace, name.local),
            )
        })
    }

    /// WSDL 2.0 members keep the QName of their declaring interface. Extending
    /// an interface never renames inherited operations into the child namespace.
    pub fn interface_contains(&self, interface: &Name, declaring: &Name) -> bool {
        self.interfaces
            .get(interface)
            .is_some_and(|set| set.contains(declaring))
    }

    pub fn interface_members(&self, interface: &Name, local: &str) -> Result<Vec<(Name, NodeRef)>> {
        let ancestors = self.interfaces.get(interface).ok_or_else(|| {
            ConversionError::new("WSDL_REFERENCE", "Binding interface is not declared")
        })?;
        let mut members = Vec::new();
        for name in ancestors {
            let declaration = self.declaration(Kind::Interface, name)?;
            for node in self.children(&declaration.node, WSDL20, local)? {
                members.push((name.clone(), node));
            }
        }
        Ok(members)
    }

    pub fn relative(&self, node: &NodeRef, path: &str, sources: &Sources) -> Result<String> {
        let mut ancestors = Vec::new();
        let mut current = Some(node.id);
        while let Some(id) = current {
            let reference = NodeRef {
                uri: node.uri.clone(),
                id,
            };
            current = self.node(&reference)?.parent;
            ancestors.push(reference);
        }
        let mut base = node.uri.to_string();
        for ancestor in ancestors.into_iter().rev() {
            if let Some(value) = self
                .node(&ancestor)?
                .attribute_ns(xml::XML_NAMESPACE, "base")
            {
                base = sources.relative(&base, value)?;
            }
        }
        sources.relative(&base, path)
    }

    fn load_document(&mut self, uri: &str, sources: &mut Sources, depth: usize) -> Result<()> {
        if self.documents.contains_key(uri) {
            return Ok(());
        }
        if depth > sources.limits.max_depth {
            return Err(ConversionError::new(
                "WSDL_IMPORT_DEPTH",
                "Document import graph exceeds depth limit",
            ));
        }
        let document = xml::parse(sources, uri)?;
        self.documents.insert(uri.into(), document);
        let root = self.root(uri)?;
        let node = self.node(&root)?;
        if node.is(XSD, "schema") {
            return Ok(());
        }
        let namespace = if node.is(WSDL11, "definitions") {
            WSDL11
        } else if node.is(WSDL20, "description") {
            WSDL20
        } else {
            return Err(ConversionError::new(
                "WSDL_IMPORT_ROOT",
                format!(
                    "Unsupported imported XML root {{{}}}{}",
                    node.name.namespace, node.name.local
                ),
            ));
        };
        let target = node.attribute("targetNamespace").unwrap_or("").to_owned();
        if namespace == WSDL20 && target.is_empty()
            || !target.is_empty() && url::Url::parse(&target).is_err()
        {
            return Err(ConversionError::new(
                "WSDL_NAMESPACE",
                "WSDL targetNamespace must be absolute; WSDL 2.0 requires it",
            ));
        }
        for (local, kind) in [
            ("message", Kind::Message),
            ("portType", Kind::PortType),
            ("interface", Kind::Interface),
            ("binding", Kind::Binding),
            ("service", Kind::Service),
        ] {
            for child in self.children(&root, namespace, local)? {
                let name = self
                    .node(&child)?
                    .attribute("name")
                    .ok_or_else(|| {
                        ConversionError::new("WSDL_NAME", format!("{local} requires name"))
                    })?
                    .to_owned();
                self.insert(
                    kind,
                    Name::new(&target, &name),
                    Declaration {
                        node: child,
                        schema: None,
                    },
                )?;
            }
        }
        for import in self
            .children(&root, namespace, "import")?
            .into_iter()
            .chain(self.children(&root, namespace, "include")?)
        {
            let node = self.node(&import)?;
            let include = node.name.local == "include";
            let expected = if include {
                target.clone()
            } else {
                node.attribute("namespace").unwrap_or("").to_owned()
            };
            let path = node
                .attribute("location")
                .map(str::to_owned)
                .unwrap_or_else(|| expected.clone());
            if path.is_empty() {
                return Err(ConversionError::new(
                    "WSDL_IMPORT_LOCATION",
                    "Supply import location or a virtual file keyed by its namespace URI",
                ));
            }
            let dependency = self.relative(&import, &path, sources)?;
            self.load_document(&dependency, sources, depth + 1)?;
            let imported_root = self.root(&dependency)?;
            let actual = self
                .node(&imported_root)?
                .attribute("targetNamespace")
                .unwrap_or("");
            if actual != expected {
                return Err(ConversionError::new(
                    "WSDL_IMPORT_NAMESPACE",
                    format!("Expected namespace {expected}, found {actual} in {dependency}"),
                ));
            }
            if self.node(&imported_root)?.is(XSD, "schema") {
                self.load_schema(imported_root, Some(expected), sources, depth + 1)?;
            }
        }
        for types in self.children(&root, namespace, "types")? {
            for schema in self.children(&types, XSD, "schema")? {
                self.load_schema(schema, None, sources, depth + 1)?;
            }
        }
        Ok(())
    }

    fn load_schema(
        &mut self,
        root: NodeRef,
        adopted: Option<String>,
        sources: &mut Sources,
        depth: usize,
    ) -> Result<()> {
        if depth > sources.limits.max_depth {
            return Err(ConversionError::new(
                "XSD_IMPORT_DEPTH",
                "Schema import graph exceeds depth limit",
            ));
        }
        let node = self.node(&root)?;
        let namespace = node
            .attribute("targetNamespace")
            .map(str::to_owned)
            .or(adopted)
            .unwrap_or_default();
        if !namespace.is_empty() && url::Url::parse(&namespace).is_err() {
            return Err(ConversionError::new(
                "XSD_NAMESPACE",
                "XSD targetNamespace must be absolute",
            ));
        }
        if !self
            .schema_instances
            .insert((root.clone(), namespace.clone()))
        {
            return Ok(());
        }
        let node = self.node(&root)?;
        let schema = Schema {
            namespace: namespace.clone(),
            chameleon: node.attribute("targetNamespace").is_none(),
            element_qualified: node.attribute("elementFormDefault") == Some("qualified"),
            attribute_qualified: node.attribute("attributeFormDefault") == Some("qualified"),
        };
        let index = self.schemas.len();
        self.schemas.push(schema);
        for (local, kind) in [
            ("simpleType", Kind::Type),
            ("complexType", Kind::Type),
            ("element", Kind::Element),
            ("attribute", Kind::Attribute),
            ("group", Kind::Group),
            ("attributeGroup", Kind::AttributeGroup),
        ] {
            for child in self.children(&root, XSD, local)? {
                let name = self
                    .node(&child)?
                    .attribute("name")
                    .ok_or_else(|| {
                        ConversionError::new("XSD_NAME", format!("Global {local} requires name"))
                    })?
                    .to_owned();
                self.insert(
                    kind,
                    Name::new(&namespace, &name),
                    Declaration {
                        node: child,
                        schema: Some(index),
                    },
                )?;
            }
        }
        for local in ["import", "include", "redefine", "override"] {
            for import in self.children(&root, XSD, local)? {
                if matches!(local, "redefine" | "override") {
                    return Err(ConversionError::new(
                        "XSD_REDEFINITION",
                        "XSD redefine/override requires a reviewed schema replacement before conversion",
                    ));
                }
                let node = self.node(&import)?;
                let expected = if local == "include" {
                    namespace.clone()
                } else {
                    node.attribute("namespace").unwrap_or("").to_owned()
                };
                let path = node
                    .attribute("schemaLocation")
                    .map(str::to_owned)
                    .unwrap_or_else(|| expected.clone());
                if path.is_empty() {
                    return Err(ConversionError::new(
                        "XSD_IMPORT_LOCATION",
                        "Supply schemaLocation or a virtual file keyed by its namespace URI",
                    ));
                }
                let dependency = self.relative(&import, &path, sources)?;
                self.load_document(&dependency, sources, depth + 1)?;
                let dependency_root = self.root(&dependency)?;
                let dependency_node = self.node(&dependency_root)?;
                if !dependency_node.is(XSD, "schema") {
                    return Err(ConversionError::new(
                        "XSD_IMPORT_ROOT",
                        "XSD imports must reference XML Schema documents",
                    ));
                }
                let actual = dependency_node.attribute("targetNamespace").unwrap_or("");
                if actual != expected && !(local == "include" && actual.is_empty()) {
                    return Err(ConversionError::new(
                        "XSD_IMPORT_NAMESPACE",
                        format!("Expected namespace {expected}, found {actual} in {dependency}"),
                    ));
                }
                self.load_schema(
                    dependency_root,
                    if local == "include" {
                        Some(expected)
                    } else {
                        None
                    },
                    sources,
                    depth + 1,
                )?;
            }
        }
        Ok(())
    }

    fn insert(&mut self, kind: Kind, name: Name, declaration: Declaration) -> Result<()> {
        let key = (kind, name);
        if let Some(previous) = self.declarations.get(&key) {
            if previous.node == declaration.node {
                return Ok(());
            }
            return Err(ConversionError::new(
                "WSDL_DUPLICATE_DECLARATION",
                format!("Duplicate {kind:?} {{{}}}{}", key.1.namespace, key.1.local),
            ));
        }
        self.declarations.insert(key, declaration);
        Ok(())
    }
}
