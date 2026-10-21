//! Bounded XML syntax and namespace handling shared by WSDL and XSD adapters.
//! This reader has no entity loader, filesystem access, or network resolver.
use crate::{ConversionError, Result, source::Sources};
use octri_parser::{Address, Diagnostic, Location, Severity};
use quick_xml::{
    NsReader, XmlVersion,
    events::{BytesStart, Event},
    name::{QName, ResolveResult},
};
use std::{collections::BTreeMap, sync::Arc};

pub(super) const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Name {
    pub namespace: String,
    pub local: String,
}

impl Name {
    pub fn new(namespace: &str, local: &str) -> Self {
        Self {
            namespace: namespace.into(),
            local: local.into(),
        }
    }
}

#[derive(Debug)]
pub(super) struct Node {
    pub name: Name,
    pub attributes: BTreeMap<Name, String>,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
    pub text: String,
    pub namespaces: Arc<BTreeMap<String, String>>,
    pub location: Location,
}

impl Node {
    pub fn attribute(&self, local: &str) -> Option<&str> {
        self.attribute_ns("", local)
    }
    pub fn attribute_ns(&self, namespace: &str, local: &str) -> Option<&str> {
        self.attributes
            .get(&Name::new(namespace, local))
            .map(String::as_str)
    }
    pub fn is(&self, namespace: &str, local: &str) -> bool {
        self.name.namespace == namespace && self.name.local == local
    }

    pub fn qname(&self, value: &str) -> Result<Name> {
        let value = value.trim_matches([' ', '\t', '\r', '\n']);
        let (prefix, local) = split_name(value)?;
        let namespace = match self.namespaces.get(prefix) {
            Some(namespace) => namespace.as_str(),
            None if prefix.is_empty() => "",
            None => {
                return Err(failure(
                    &self.location,
                    "XML_NAMESPACE",
                    format!("Undeclared QName prefix {prefix}"),
                ));
            }
        };
        Ok(Name::new(namespace, local))
    }
}

#[derive(Debug)]
pub(super) struct Document {
    pub nodes: Vec<Node>,
    pub root: usize,
}

impl Document {
    pub fn children<'a>(
        &'a self,
        node: usize,
        namespace: &'a str,
        local: &'a str,
    ) -> impl Iterator<Item = usize> + 'a {
        self.nodes[node]
            .children
            .iter()
            .copied()
            .filter(move |id| self.nodes[*id].is(namespace, local))
    }
}

pub(super) fn parse(sources: &mut Sources, uri: &str) -> Result<Document> {
    let text = sources.text(uri)?.to_owned();
    parse_text(sources, uri, &text, None)
}

/// An XML document embedded in a YAML scalar retains that scalar's location.
/// Its private identity establishes import scope without replacing source bytes.
pub(super) fn parse_embedded(
    sources: &mut Sources,
    uri: &str,
    text: &str,
    physical: &Address,
) -> Result<Document> {
    let location = sources.physical_location(physical);
    parse_text(sources, uri, text, Some(&location))
}

fn parse_text(
    sources: &mut Sources,
    uri: &str,
    text: &str,
    embedded_location: Option<&Location>,
) -> Result<Document> {
    if text.len() > sources.limits.max_document_bytes {
        return Err(ConversionError::new(
            "LIMIT_XML_BYTES",
            "XML document exceeds configured byte limit",
        ));
    }
    if text.chars().any(|ch| !xml_character(ch)) {
        return Err(ConversionError::new(
            "XML_CHARACTER",
            "Source contains a character forbidden by XML 1.0",
        ));
    }
    let mut reader = NsReader::from_str(text);
    reader.config_mut().check_comments = true;
    let mut nodes: Vec<Node> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut root = None;
    let mut cursor = Cursor::default();
    let mut model_nodes = 0usize;
    let mut expanded_bytes = 0usize;
    let mut declaration = false;
    loop {
        let offset = usize::try_from(reader.buffer_position())
            .map_err(|_| ConversionError::new("XML_OFFSET", "XML offset cannot be represented"))?;
        cursor.advance(text, offset);
        let pointer = format!("/xml/{}", nodes.len());
        let location = embedded_location.cloned().unwrap_or_else(|| Location {
            address: Address {
                uri: uri.into(),
                pointer,
            },
            line: Some(cursor.line),
            column: Some(cursor.column),
            offset: Some(offset),
        });
        let event = reader
            .read_event()
            .map_err(|error| failure(&location, "XML_SYNTAX", error.to_string()))?;
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                if stack.len() > sources.limits.max_depth {
                    return Err(failure(
                        &location,
                        "LIMIT_XML_DEPTH",
                        "XML nesting exceeds depth limit",
                    ));
                }
                let parent_namespaces = stack
                    .last()
                    .map(|id| nodes[*id].namespaces.clone())
                    .unwrap_or_else(|| {
                        Arc::new(BTreeMap::from([("xml".into(), XML_NAMESPACE.into())]))
                    });
                let mut node = element(
                    &reader,
                    start,
                    parent_namespaces,
                    location.clone(),
                    &mut expanded_bytes,
                    sources.limits.max_total_bytes,
                )?;
                node.parent = stack.last().copied();
                model_nodes = model_nodes.saturating_add(1 + node.attributes.len());
                expanded_bytes = expanded_bytes.saturating_add(
                    node.name.namespace.len()
                        + node.name.local.len()
                        + node
                            .attributes
                            .iter()
                            .map(|(key, value)| key.namespace.len() + key.local.len() + value.len())
                            .sum::<usize>(),
                );
                if model_nodes > sources.limits.max_nodes
                    || expanded_bytes > sources.limits.max_total_bytes
                {
                    return Err(failure(
                        &node.location,
                        "LIMIT_XML_MODEL",
                        "Expanded XML model exceeds configured limits",
                    ));
                }
                let id = nodes.len();
                if let Some(parent) = stack.last() {
                    nodes[*parent].children.push(id);
                } else if root.replace(id).is_some() {
                    return Err(failure(
                        &node.location,
                        "XML_ROOT",
                        "XML document requires exactly one root element",
                    ));
                }
                nodes.push(node);
                if matches!(event, Event::Start(_)) {
                    stack.push(id);
                }
            }
            Event::End(_) => {
                if stack.pop().is_none() {
                    return Err(failure(
                        &location,
                        "XML_SYNTAX",
                        "Unexpected closing element",
                    ));
                }
            }
            Event::Text(value) => append_text(
                &mut nodes,
                &stack,
                &value.xml10_content(),
                &location,
                &mut model_nodes,
                &mut expanded_bytes,
            )?,
            Event::CData(value) => {
                if stack.is_empty() {
                    return Err(failure(
                        &location,
                        "XML_SYNTAX",
                        "CDATA must occur inside an element",
                    ));
                }
                append_text(
                    &mut nodes,
                    &stack,
                    &value.xml10_content(),
                    &location,
                    &mut model_nodes,
                    &mut expanded_bytes,
                )?;
            }
            Event::GeneralRef(value) => {
                let resolved = if let Some(ch) = value
                    .resolve_char_ref()
                    .map_err(|error| failure(&location, "XML_ENTITY", error.to_string()))?
                {
                    if !xml_character(ch) {
                        return Err(failure(
                            &location,
                            "XML_CHARACTER",
                            "Character reference is forbidden by XML 1.0",
                        ));
                    }
                    ch.to_string()
                } else {
                    let name = value.as_ref();
                    match name { "lt"=>"<", "gt"=>">", "amp"=>"&", "apos"=>"'", "quot"=>"\"", _=>return Err(failure(&location,"XML_ENTITY", "Only predefined XML entities and character references are accepted")) }.into()
                };
                if stack.is_empty() {
                    return Err(failure(
                        &location,
                        "XML_SYNTAX",
                        "Entity references must occur inside an element",
                    ));
                }
                append_text(
                    &mut nodes,
                    &stack,
                    &resolved,
                    &location,
                    &mut model_nodes,
                    &mut expanded_bytes,
                )?;
            }
            Event::Decl(value) => {
                if declaration
                    || root.is_some()
                    || offset != 0 && !(offset == 3 && text.starts_with('\u{feff}'))
                {
                    return Err(failure(
                        &location,
                        "XML_DECLARATION",
                        "XML declaration must occur once at document start",
                    ));
                }
                declaration = true;
                if value
                    .version()
                    .map_err(|error| failure(&location, "XML_DECLARATION", error.to_string()))?
                    .as_ref()
                    != "1.0"
                {
                    return Err(failure(
                        &location,
                        "XML_VERSION",
                        "Supply an XML 1.0 document",
                    ));
                }
                if let Some(encoding) = value.encoding() {
                    let encoding = encoding
                        .map_err(|error| failure(&location, "XML_ENCODING", error.to_string()))?;
                    if !encoding.eq_ignore_ascii_case("utf-8")
                        && !encoding.eq_ignore_ascii_case("us-ascii")
                    {
                        return Err(failure(
                            &location,
                            "XML_ENCODING",
                            "Source text must declare UTF-8 or US-ASCII; decode original bytes before conversion",
                        ));
                    }
                    if encoding.eq_ignore_ascii_case("us-ascii") && !text.is_ascii() {
                        return Err(failure(
                            &location,
                            "XML_ENCODING",
                            "US-ASCII declaration conflicts with source characters",
                        ));
                    }
                }
            }
            Event::DocType(_) => {
                return Err(failure(
                    &location,
                    "XML_DTD_DISABLED",
                    "DTD declarations are not accepted; provide a self-contained WSDL/XSD with explicit virtual imports",
                ));
            }
            Event::PI(_) | Event::Comment(_) => {}
            Event::Eof => break,
        }
        if model_nodes > sources.limits.max_nodes || expanded_bytes > sources.limits.max_total_bytes
        {
            return Err(failure(
                &location,
                "LIMIT_XML_MODEL",
                "Expanded XML model exceeds configured limits",
            ));
        }
    }
    if !stack.is_empty() {
        return Err(ConversionError::new("XML_SYNTAX", "Unclosed XML element"));
    }
    let root =
        root.ok_or_else(|| ConversionError::new("XML_ROOT", "XML document has no root element"))?;
    if embedded_location.is_some() {
        sources.charge_derived_nodes(model_nodes)?;
    } else {
        sources.register_model_nodes(model_nodes)?;
    }
    for node in nodes.iter().filter(|_| embedded_location.is_none()) {
        sources.register_byte_location(
            uri,
            &node.location.address.pointer,
            node.location.offset.unwrap_or(0),
            node.location.line.unwrap_or(1),
            node.location.column.unwrap_or(1),
        );
    }
    Ok(Document { nodes, root })
}

fn element(
    reader: &NsReader<&[u8]>,
    start: &BytesStart<'_>,
    inherited: Arc<BTreeMap<String, String>>,
    location: Location,
    expanded_bytes: &mut usize,
    byte_limit: usize,
) -> Result<Node> {
    let (namespace, local) = reader.resolver().resolve_element(start.name());
    let name = resolved(namespace, local.as_ref(), &location)?;
    split_name(start.name().as_ref())?;
    let mut attributes = BTreeMap::new();
    let mut namespaces = inherited;
    for attribute in start.attributes() {
        let attribute =
            attribute.map_err(|error| failure(&location, "XML_ATTRIBUTE", error.to_string()))?;
        let raw = attribute.key.as_ref();
        split_name(raw)?;
        if attribute.value.contains('<') {
            return Err(failure(
                &location,
                "XML_ATTRIBUTE",
                "Literal '<' is forbidden in XML attribute values",
            ));
        }
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|error| failure(&location, "XML_ATTRIBUTE", error.to_string()))?
            .into_owned();
        if value.chars().any(|ch| !xml_character(ch)) {
            return Err(failure(
                &location,
                "XML_CHARACTER",
                "Attribute contains a forbidden XML character",
            ));
        }
        if raw == "xmlns" || raw.starts_with("xmlns:") {
            let prefix = raw.strip_prefix("xmlns:").unwrap_or("");
            if !prefix.is_empty() && value.is_empty() {
                return Err(failure(
                    &location,
                    "XML_NAMESPACE",
                    "XML 1.0 forbids empty prefixed namespace bindings",
                ));
            }
            if Arc::strong_count(&namespaces) > 1 {
                *expanded_bytes = expanded_bytes.saturating_add(
                    namespaces
                        .iter()
                        .map(|(key, value)| key.len() + value.len())
                        .sum::<usize>(),
                );
            }
            *expanded_bytes = expanded_bytes.saturating_add(prefix.len() + value.len());
            if *expanded_bytes > byte_limit {
                return Err(failure(
                    &location,
                    "LIMIT_XML_NAMESPACES",
                    "Expanded namespace scopes exceed byte limit",
                ));
            }
            Arc::make_mut(&mut namespaces).insert(prefix.into(), value);
        } else {
            let (namespace, local) = reader
                .resolver()
                .resolve_attribute(QName(attribute.key.as_ref()));
            let key = resolved(namespace, local.as_ref(), &location)?;
            if attributes.insert(key, value).is_some() {
                return Err(failure(
                    &location,
                    "XML_ATTRIBUTE",
                    "Duplicate expanded attribute name",
                ));
            }
        }
    }
    Ok(Node {
        name,
        attributes,
        children: Vec::new(),
        parent: None,
        text: String::new(),
        namespaces,
        location,
    })
}

fn resolved(namespace: ResolveResult<'_>, local: &str, location: &Location) -> Result<Name> {
    let namespace = match namespace {
        ResolveResult::Bound(value) => value.as_ref().to_owned(),
        ResolveResult::Unbound => String::new(),
        ResolveResult::Unknown(prefix) => {
            return Err(failure(
                location,
                "XML_NAMESPACE",
                format!("Undeclared namespace prefix {prefix}"),
            ));
        }
    };
    Ok(Name {
        namespace,
        local: local.into(),
    })
}

fn append_text(
    nodes: &mut [Node],
    stack: &[usize],
    text: &str,
    location: &Location,
    count: &mut usize,
    bytes: &mut usize,
) -> Result<()> {
    if let Some(id) = stack.last() {
        if nodes[*id].text.is_empty() && !text.is_empty() {
            *count = count.saturating_add(1);
        }
        *bytes = bytes.saturating_add(text.len());
        nodes[*id].text.push_str(text);
    } else if !text
        .trim_matches([' ', '\t', '\n', '\r', '\u{feff}'])
        .is_empty()
    {
        return Err(failure(
            location,
            "XML_ROOT",
            "Non-whitespace text outside root element",
        ));
    }
    Ok(())
}

fn split_name(value: &str) -> Result<(&str, &str)> {
    let (prefix, local) = value.split_once(':').unwrap_or(("", value));
    if value.starts_with(':')
        || !prefix.is_empty() && !ncname(prefix)
        || !ncname(local)
        || local.contains(':')
    {
        return Err(ConversionError::new(
            "XML_NAME",
            format!("Invalid XML qualified name {value}"),
        ));
    }
    Ok((prefix, local))
}

pub(super) fn ncname(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(name_start) && chars.all(|ch| name_start(ch) || matches!(ch, '-' | '.' | '0'..='9' | '\u{b7}' | '\u{300}'..='\u{36f}' | '\u{203f}'..='\u{2040}'))
}

fn name_start(ch: char) -> bool {
    matches!(ch, '_' | 'A'..='Z' | 'a'..='z' | '\u{c0}'..='\u{d6}' | '\u{d8}'..='\u{f6}' | '\u{f8}'..='\u{2ff}' | '\u{370}'..='\u{37d}' | '\u{37f}'..='\u{1fff}' | '\u{200c}'..='\u{200d}' | '\u{2070}'..='\u{218f}' | '\u{2c00}'..='\u{2fef}' | '\u{3001}'..='\u{d7ff}' | '\u{f900}'..='\u{fdcf}' | '\u{fdf0}'..='\u{fffd}' | '\u{10000}'..='\u{effff}')
}
fn xml_character(ch: char) -> bool {
    matches!(ch, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}

#[derive(Debug)]
struct Cursor {
    offset: usize,
    line: usize,
    column: usize,
    previous_cr: bool,
}
impl Default for Cursor {
    fn default() -> Self {
        Self {
            offset: 0,
            line: 1,
            column: 1,
            previous_cr: false,
        }
    }
}
impl Cursor {
    fn advance(&mut self, text: &str, offset: usize) {
        if let Some(consumed) = text.get(self.offset..offset) {
            for ch in consumed.chars() {
                if ch == '\r' {
                    self.line += 1;
                    self.column = 1;
                } else if ch == '\n' {
                    if !self.previous_cr {
                        self.line += 1;
                    }
                    self.column = 1;
                } else {
                    self.column += 1;
                }
                self.previous_cr = ch == '\r';
            }
            self.offset = offset;
        }
    }
}

fn failure(location: &Location, code: &str, message: impl Into<String>) -> ConversionError {
    let message = message.into();
    ConversionError {
        code: code.into(),
        message: message.clone(),
        diagnostics: vec![Diagnostic {
            code: code.into(),
            severity: Severity::Error,
            message,
            location: location.clone(),
            related: Vec::new(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, ConversionRequest};

    fn sources(text: &str) -> Result<Sources> {
        Sources::new(&ConversionRequest {
            input: octri_parser::Input {
                uri: "memory://xml/source.wsdl".into(),
                content: Some(text.into()),
                generated: false,
            },
            files: Default::default(),
            config: Config::default(),
        })
    }

    #[test]
    fn embedded_xml_uses_derived_nodes_and_physical_xml_uses_source_nodes() -> Result<()> {
        let text = "<root><child/></root>";
        let mut input = sources(text)?;
        input.limits.max_nodes = 3;
        input.register_model_nodes(2)?;
        input.admit_derived("xml:body", text.len(), 0)?;
        let physical = Address {
            uri: input.root.clone(),
            pointer: "/body".into(),
        };
        let document = parse_embedded(&mut input, "memory://xml/derived.xsd", text, &physical)?;
        assert_eq!(document.nodes.len(), 2);
        input.register_model_nodes(1)?;
        assert_eq!(
            input
                .charge_derived_nodes(2)
                .err()
                .ok_or_else(|| ConversionError::new("TEST", "Derived nodes not charged"))?
                .code,
            "SCHEMA_DERIVED_LIMIT"
        );

        let mut input = sources(text)?;
        input.limits.max_nodes = 3;
        input.register_model_nodes(2)?;
        assert_eq!(
            parse(&mut input, "memory://xml/source.wsdl")
                .err()
                .ok_or_else(|| ConversionError::new("TEST", "Physical source nodes not charged"))?
                .code,
            "LIMIT_NODES"
        );
        Ok(())
    }

    #[test]
    fn namespace_scopes_qnames_attributes_and_locations_are_physical() -> Result<()> {
        let text = "<?xml version=\"1.0\"?>\r\n<d xmlns=\"urn:outer\" xmlns:t=\"urn:types\">\r\n  <child t:type=\"t:Thing\" plain=\"A&amp;B\"><inner xmlns:t=\"urn:inner\" ref=\"t:Value\"/></child>\r\n</d>";
        let mut input = sources(text)?;
        let document = parse(&mut input, "memory://xml/source.wsdl")?;
        let child = &document.nodes[1];
        assert_eq!(child.name, Name::new("urn:outer", "child"));
        assert_eq!(child.attribute("plain"), Some("A&B"));
        assert_eq!(
            child.qname(
                child
                    .attribute_ns("urn:types", "type")
                    .ok_or_else(|| ConversionError::new("TEST", "missing type"))?
            )?,
            Name::new("urn:types", "Thing")
        );
        assert_eq!(
            document.nodes[2].qname("t:Value")?,
            Name::new("urn:inner", "Value")
        );
        assert_eq!(child.location.line, Some(3));
        assert_eq!(child.location.column, Some(3));
        assert_eq!(child.location.offset, text.find("<child"));
        assert_eq!(
            input.location("memory://xml/source.wsdl", "/xml/1"),
            child.location
        );
        assert_eq!(
            document
                .children(document.root, "urn:outer", "child")
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    fn entities_text_and_cdata_remain_inert() -> Result<()> {
        let mut input = sources("<root>A&amp;B&#x20;<![CDATA[<literal/>]]></root>")?;
        let document = parse(&mut input, "memory://xml/source.wsdl")?;
        assert_eq!(document.nodes[document.root].text, "A&B <literal/>");
        assert!(document.nodes[document.root].children.is_empty());
        Ok(())
    }

    #[test]
    fn invalid_documents_and_external_entity_declarations_fail() -> Result<()> {
        for text in [
            "<!DOCTYPE d SYSTEM 'file:///etc/passwd'><d/>",
            "<!DOCTYPE d [<!ENTITY e 'expanded'>]><d>&e;</d>",
            "<d>&unknown;</d>",
            "<d>&#0;</d>",
            "<d/><d/>",
            "<d>",
            "<d><b></d>",
            "<p:d/>",
            "<:d/>",
            "<d xmlns:a='urn:same' xmlns:b='urn:same' a:x='1' b:x='2'/>",
            "<d value='<bad'/>",
            "<?xml version='1.1'?><d/>",
            "<?xml version='1.0' encoding='UTF-16'?><d/>",
        ] {
            let mut input = sources(text)?;
            assert!(
                parse(&mut input, "memory://xml/source.wsdl").is_err(),
                "accepted {text}"
            );
        }
        Ok(())
    }

    #[test]
    fn nesting_and_namespace_expansion_obey_limits() -> Result<()> {
        let mut input = sources("<a><b><c/></b></a>")?;
        input.limits.max_depth = 1;
        assert!(parse(&mut input, "memory://xml/source.wsdl").is_err());
        let large = "n".repeat(300);
        let text =
            format!("<a xmlns:p='urn:{large}'><b xmlns:q='urn:q'><c xmlns:r='urn:r'/></b></a>");
        let mut input = sources(&text)?;
        input.limits.max_total_bytes = text.len() + 50;
        assert!(parse(&mut input, "memory://xml/source.wsdl").is_err());
        Ok(())
    }
}
