//! XML Schema projection. XML-only constraints receive explicit fidelity decisions.
use super::super::xml::Name;
use super::catalog::{Catalog, Declaration, Kind, NodeRef, XSD};
use crate::{
    Classification, ConversionError, Result,
    source::{Sources, child, hash},
    trace::Trace,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Projection<'a> {
    catalog: &'a Catalog,
    sources: &'a Sources,
    trace: &'a mut Trace,
    names: BTreeMap<(Kind, Name), String>,
    active: BTreeSet<(NodeRef, usize)>,
    nodes: usize,
}

impl<'a> Projection<'a> {
    pub fn new(catalog: &'a Catalog, sources: &'a Sources, trace: &'a mut Trace) -> Self {
        let names = catalog
            .declarations
            .keys()
            .filter(|(kind, _)| matches!(kind, Kind::Type | Kind::Element | Kind::Attribute))
            .map(|key| (key.clone(), component_name(key.0, &key.1)))
            .collect();
        Self {
            catalog,
            sources,
            trace,
            names,
            active: BTreeSet::new(),
            nodes: 0,
        }
    }

    pub fn components(&mut self) -> Result<Map<String, Value>> {
        let mut result = Map::new();
        for (key, name) in self.names.clone() {
            let declaration = self.catalog.declaration(key.0, &key.1)?.clone();
            let schema = self.declaration(&declaration, key.0, 0)?;
            if result.insert(name, schema).is_some() {
                return Err(ConversionError::new(
                    "XSD_COMPONENT_COLLISION",
                    "Distinct XML declarations produced the same component name",
                ));
            }
        }
        Ok(result)
    }

    pub fn reference(&self, kind: Kind, name: &Name) -> Result<Value> {
        reference(self.catalog, kind, name)
    }

    pub fn declaration(
        &mut self,
        declaration: &Declaration,
        kind: Kind,
        depth: usize,
    ) -> Result<Value> {
        let site = declaration.schema.ok_or_else(|| {
            ConversionError::new("XSD_CONTEXT", "Declaration is not part of an XML Schema")
        })?;
        if kind == Kind::Type {
            self.type_node(&declaration.node, site, depth)
        } else {
            self.element(
                &declaration.node,
                site,
                kind == Kind::Attribute,
                true,
                depth,
            )
        }
    }

    fn tick(&mut self, depth: usize) -> Result<()> {
        self.nodes += 1;
        if self.nodes > self.sources.limits.max_nodes || depth > self.sources.limits.max_depth {
            return Err(ConversionError::new(
                "XSD_EXPANSION_LIMIT",
                "Projected XML Schema exceeds node or depth limit",
            ));
        }
        Ok(())
    }

    fn name(&self, node: &NodeRef, value: &str, site: usize) -> Result<Name> {
        let mut name = self.catalog.node(node)?.qname(value)?;
        let schema = &self.catalog.schemas[site];
        if name.namespace.is_empty() && schema.chameleon {
            name.namespace = schema.namespace.clone();
        }
        Ok(name)
    }

    fn element(
        &mut self,
        at: &NodeRef,
        site: usize,
        attribute: bool,
        global: bool,
        depth: usize,
    ) -> Result<Value> {
        self.tick(depth)?;
        let node = self.catalog.node(at)?;
        let mut schema = if let Some(reference) = node.attribute("ref") {
            self.reference(
                if attribute {
                    Kind::Attribute
                } else {
                    Kind::Element
                },
                &self.name(at, reference, site)?,
            )?
        } else if let Some(kind) = node.attribute("type") {
            self.type_reference(at, kind, site)?
        } else {
            let types = self
                .catalog
                .children(at, XSD, "complexType")?
                .into_iter()
                .chain(self.catalog.children(at, XSD, "simpleType")?)
                .collect::<Vec<_>>();
            if types.len() > 1 {
                return Err(ConversionError::new(
                    "XSD_TYPE",
                    "Element/attribute declares multiple anonymous types",
                ));
            }
            if let Some(kind) = types.first() {
                self.type_node(kind, site, depth + 1)?
            } else {
                json!({})
            }
        };
        if let Some(name) = node.attribute("name") {
            let context = &self.catalog.schemas[site];
            let qualified = global
                || node.attribute("form") == Some("qualified")
                || node.attribute("form").is_none()
                    && if attribute {
                        context.attribute_qualified
                    } else {
                        context.element_qualified
                    };
            let mut xml = json!({"name":name});
            if qualified && !context.namespace.is_empty() {
                xml["namespace"] = json!(context.namespace);
            }
            if attribute {
                xml["attribute"] = json!(true);
            }
            schema["xml"] = xml;
        }
        if node
            .attribute("nillable")
            .is_some_and(|value| matches!(value, "true" | "1"))
        {
            schema = json!({"anyOf":[schema,{"type":"null"}],"x-xsd-nillable":true});
            self.loss(at,"XSD_NIL_SERIALIZATION","OpenAPI null projection does not require a particular xsi:nil encoding; review XML serializer behavior");
        }
        for (source, target) in [("default", "default"), ("fixed", "const")] {
            if let Some(value) = node.attribute(source) {
                if let Some(value) = literal(&schema, value) {
                    schema[target] = value;
                } else {
                    self.loss(at,"XSD_LEXICAL_VALUE",format!("{source} lexical value requires the original XML type evaluator; it is not emitted as a mismatched JSON instance"));
                }
            }
        }
        for key in ["key", "unique", "keyref", "alternative"] {
            for child in self.catalog.children(at, XSD, key)? {
                self.loss(
                    &child,
                    "XSD_IDENTITY_CONSTRAINT",
                    format!("XML {key} constraint is not enforced by the OpenAPI Schema Object"),
                );
            }
        }
        self.documentation(at, &mut schema)?;
        Ok(schema)
    }

    fn type_node(&mut self, at: &NodeRef, site: usize, depth: usize) -> Result<Value> {
        self.tick(depth)?;
        let node = self.catalog.node(at)?;
        let mut schema = if node.is(XSD, "simpleType") {
            self.simple(at, site, depth + 1)?
        } else if node.is(XSD, "complexType") {
            self.complex(at, site, depth + 1)?
        } else {
            return Err(ConversionError::new(
                "XSD_TYPE",
                "Expected simpleType or complexType",
            ));
        };
        if node
            .attribute("abstract")
            .is_some_and(|value| matches!(value, "true" | "1"))
        {
            self.loss(at,"XSD_ABSTRACT_TYPE","Abstract XML types require xsi:type or substitution-group validation beyond an OpenAPI XML projection");
        }
        self.documentation(at, &mut schema)?;
        Ok(schema)
    }

    fn simple(&mut self, at: &NodeRef, site: usize, depth: usize) -> Result<Value> {
        self.tick(depth)?;
        if let Some(restriction) = self.catalog.children(at, XSD, "restriction")?.first() {
            let node = self.catalog.node(restriction)?;
            let base = if let Some(base) = node.attribute("base") {
                self.type_reference(restriction, base, site)?
            } else if let Some(inline) = self
                .catalog
                .children(restriction, XSD, "simpleType")?
                .first()
            {
                self.simple(inline, site, depth + 1)?
            } else {
                return Err(ConversionError::new(
                    "XSD_RESTRICTION",
                    "Simple restriction requires a base type",
                ));
            };
            let mut schema = json!({"allOf":[base.clone()]});
            let mut enumeration = Vec::new();
            let mut exact_enum = true;
            for id in &node.children {
                let child = NodeRef {
                    uri: at.uri.clone(),
                    id: *id,
                };
                let facet = self.catalog.node(&child)?;
                if facet.is(XSD, "annotation") {
                    continue;
                }
                let value = facet
                    .attribute("value")
                    .ok_or_else(|| ConversionError::new("XSD_FACET", "Facet requires value"))?;
                match facet.name.local.as_str() {
                    "enumeration"=>if let Some(value)=literal(&base,value) { enumeration.push(value); } else { exact_enum=false; },
                    "minLength"|"maxLength"|"length"=> {
                        let length=value.parse::<u64>().map_err(|_|ConversionError::new("XSD_FACET","Length facet must be a nonnegative integer"))?;
                        if facet.name.local!="maxLength" { schema["minLength"]=json!(length); }
                        if facet.name.local!="minLength" { schema["maxLength"]=json!(length); }
                    }
                    "minInclusive"|"maxInclusive"|"minExclusive"|"maxExclusive"=> {
                        let key=match facet.name.local.as_str() {"minInclusive"=>"minimum","maxInclusive"=>"maximum","minExclusive"=>"exclusiveMinimum",_=>"exclusiveMaximum"};
                        if let Some(number)=exact_number(value) { schema[key]=Value::Number(number); }
                        else { self.loss(&child,"XSD_NUMERIC_PRECISION","Numeric facet cannot be emitted without changing its exact value"); }
                    }
                    _=>self.loss(&child,"XSD_FACET_PROJECTION",format!("XML facet {} remains in source; its lexical or validation semantics are not imposed by the OpenAPI projection",facet.name.local)),
                }
            }
            if !exact_enum {
                self.loss(restriction,"XSD_ENUM_PROJECTION","Enumeration requires an exact XML lexical-type evaluator; partial enumeration would wrongly exclude valid source values");
            } else if !enumeration.is_empty() {
                schema["enum"] = Value::Array(enumeration);
            }
            return Ok(schema);
        }
        if let Some(union) = self.catalog.children(at, XSD, "union")?.first() {
            let node = self.catalog.node(union)?;
            let mut members = Vec::new();
            for name in node
                .attribute("memberTypes")
                .unwrap_or("")
                .split_whitespace()
            {
                members.push(self.type_reference(union, name, site)?);
            }
            for inline in self.catalog.children(union, XSD, "simpleType")? {
                members.push(self.simple(&inline, site, depth + 1)?);
            }
            if members.is_empty() {
                return Err(ConversionError::new(
                    "XSD_UNION",
                    "Union requires member types",
                ));
            }
            return Ok(json!({"anyOf":members}));
        }
        if let Some(list) = self.catalog.children(at, XSD, "list")?.first() {
            self.loss(list,"XSD_LIST_LEXICAL_FORM","XML list is space-separated character content; item validation remains in source rather than changing it into repeated XML elements");
            return Ok(json!({"type":"string","x-xsd-list":true}));
        }
        Err(ConversionError::new(
            "XSD_SIMPLE_TYPE",
            "Simple type requires restriction, union or list",
        ))
    }

    fn complex(&mut self, at: &NodeRef, site: usize, depth: usize) -> Result<Value> {
        self.tick(depth)?;
        let mut schema = json!({"type":"object","properties":{}});
        let node = self.catalog.node(at)?;
        if node
            .attribute("mixed")
            .is_some_and(|value| matches!(value, "true" | "1"))
        {
            self.loss(
                at,
                "XSD_MIXED_CONTENT",
                "Mixed XML text/element order is not enforced by the OpenAPI object projection",
            );
        }
        self.members(at, site, &mut schema, depth + 1)?;
        if schema["x-xsd-open-content"] == true {
            schema["additionalProperties"] = json!(true);
        } else if schema.get("allOf").is_some() {
            schema["unevaluatedProperties"] = json!(false);
        } else {
            schema["additionalProperties"] = json!(false);
        }
        Ok(schema)
    }

    fn members(
        &mut self,
        at: &NodeRef,
        site: usize,
        schema: &mut Value,
        depth: usize,
    ) -> Result<()> {
        self.tick(depth)?;
        if !self.active.insert((at.clone(), site)) {
            return Err(ConversionError::new(
                "XSD_GROUP_CYCLE",
                "XML model-group expansion is cyclic",
            ));
        }
        let children = self.catalog.node(at)?.children.clone();
        for id in children {
            let child = NodeRef {
                uri: at.uri.clone(),
                id,
            };
            let node = self.catalog.node(&child)?;
            if node.name.namespace != XSD {
                self.loss(
                    &child,
                    "XSD_EXTENSION",
                    "Foreign XML Schema extension is retained in source",
                );
                continue;
            }
            match node.name.local.as_str() {
                "element"|"attribute"=> {
                    let attribute=node.name.local=="attribute";
                    if attribute && node.attribute("use")==Some("prohibited") { self.loss(&child,"XSD_PROHIBITED_ATTRIBUTE","Prohibited inherited attribute requires XML restriction validation"); continue; }
                    let name=node.attribute("name").map(str::to_owned).or_else(||node.attribute("ref").and_then(|name|self.name(&child,name,site).ok()).map(|name|name.local)).ok_or_else(||ConversionError::new("XSD_MEMBER","Element/attribute requires name or ref"))?;
                    let key=if attribute {format!("@{name}")} else {name};
                    if schema["properties"].get(&key).is_some() { self.loss(&child,"XSD_REPEATED_PARTICLE","Repeated particle with the same name has ordering/group constraints beyond the object projection"); continue; }
                    let minimum = node.attribute("minOccurs").unwrap_or("1").parse::<u64>()
                        .map_err(|_| ConversionError::new("XSD_OCCURRENCE", "minOccurs must be a nonnegative integer"))?;
                    let maximum = match node.attribute("maxOccurs").unwrap_or("1") {
                        "unbounded" => None,
                        value => Some(value.parse::<u64>().map_err(|_| ConversionError::new("XSD_OCCURRENCE", "maxOccurs must be a nonnegative integer or unbounded"))?),
                    };
                    if !attribute && maximum.is_some_and(|maximum| minimum > maximum) {
                        return Err(ConversionError::new("XSD_OCCURRENCE", "minOccurs cannot exceed maxOccurs"));
                    }
                    let mut value = if !attribute && maximum == Some(0) {
                        // A prohibited particle means absence, not an empty array.
                        json!(false)
                    } else {
                        self.element(&child, site, attribute, false, depth + 1)?
                    };
                    if !attribute && maximum != Some(0) && maximum != Some(1) {
                        value = json!({"type":"array","items":value,"minItems":minimum,"xml":{"wrapped":false}});
                        if let Some(maximum) = maximum { value["maxItems"] = json!(maximum); }
                    }
                    schema["properties"][&key]=value;
                    if if attribute {node.attribute("use")==Some("required")} else {minimum>0} {
                        if schema.get("required").is_none() { schema["required"]=json!([]); }
                        if let Some(required)=schema["required"].as_array_mut() { required.push(json!(key)); }
                    }
                }
                "sequence"|"all"=> {
                    if node.name.local=="sequence" { self.loss(&child,"XSD_ELEMENT_ORDER","XML sequence order is retained in source; OpenAPI properties do not enforce element order"); }
                    if node.attribute("maxOccurs")==Some("0") {continue;}
                    let previous=schema.get("required").cloned();
                    if node.attribute("minOccurs").is_some_and(|value|value!="1") || node.attribute("maxOccurs").is_some_and(|value|value!="1") {self.loss(&child,"XSD_GROUP_OCCURRENCE","Group occurrence constraints require original XML content-model validation");}
                    self.members(&child,site,schema,depth+1)?;
                    if node.attribute("minOccurs")==Some("0") {
                        if let Some(previous)=previous {schema["required"]=previous;} else if let Some(map)=schema.as_object_mut(){map.remove("required");}
                    }
                }
                "choice"=> {
                    self.loss(&child,"XSD_CHOICE_GROUP","XML choice and particle occurrence constraints remain in source; emitted properties do not enforce the XML content model");
                    let previous=schema.get("required").cloned();
                    self.members(&child,site,schema,depth+1)?;
                    if let Some(previous)=previous {schema["required"]=previous;} else if let Some(map)=schema.as_object_mut(){map.remove("required");}
                }
                "group"|"attributeGroup"=> {
                    if let Some(reference)=node.attribute("ref") {
                        let kind=if node.name.local=="group" {Kind::Group}else{Kind::AttributeGroup};
                        let declaration=self.catalog.declaration(kind,&self.name(&child,reference,site)?)?.clone();
                        self.members(&declaration.node,declaration.schema.unwrap_or(site),schema,depth+1)?;
                    } else { self.members(&child,site,schema,depth+1)?; }
                }
                "complexContent"=>self.members(&child,site,schema,depth+1)?,
                "extension"|"restriction"=> {
                    if let Some(base)=node.attribute("base") { self.inherit(&child,base,site,schema,depth+1)?; }
                    if node.name.local=="restriction" { self.loss(&child,"XSD_COMPLEX_RESTRICTION","XML complex-type restriction semantics require original XSD validation"); }
                    self.members(&child,site,schema,depth+1)?;
                }
                "annotation"=>{},
                "any"|"anyAttribute"=> {
                    schema["x-xsd-open-content"]=json!(true);
                    self.loss(&child,"XSD_WILDCARD","XML wildcard namespace and processContents constraints are not imposed by the OpenAPI object projection");
                }
                _=>self.loss(&child,"XSD_CONTENT_MODEL",format!("XML {} content constraint remains in source and is not enforced by the OpenAPI object projection",node.name.local)),
            }
        }
        self.active.remove(&(at.clone(), site));
        Ok(())
    }

    fn inherit(
        &mut self,
        at: &NodeRef,
        base: &str,
        site: usize,
        schema: &mut Value,
        depth: usize,
    ) -> Result<()> {
        let name = self.name(at, base, site)?;
        let mut inherited = if name.namespace == XSD {
            self.type_reference(at, base, site)?
        } else {
            let declaration = self.catalog.declaration(Kind::Type, &name)?.clone();
            self.declaration(&declaration, Kind::Type, depth + 1)?
        };
        // allOf cannot extend a closed base: its additionalProperties would
        // reject new fields. Flatten fields once, then close the final model.
        if let Some(map) = inherited.as_object_mut() {
            map.remove("additionalProperties");
            map.remove("unevaluatedProperties");
        }
        if let Some(properties) = inherited
            .get_mut("properties")
            .and_then(Value::as_object_mut)
        {
            for (name, value) in std::mem::take(properties) {
                if let Some(existing) = schema["properties"].get_mut(&name) {
                    *existing = json!({"allOf":[existing.clone(),value]});
                } else {
                    schema["properties"][name] = value;
                }
            }
        }
        if let Some(required) = inherited.get_mut("required").and_then(Value::as_array_mut) {
            for name in std::mem::take(required) {
                if schema.get("required").is_none() {
                    schema["required"] = json!([]);
                }
                if let Some(values) = schema["required"].as_array_mut()
                    && !values.contains(&name)
                {
                    values.push(name);
                }
            }
        }
        if schema.get("allOf").is_none() {
            schema["allOf"] = json!([]);
        }
        if let Some(values) = schema["allOf"].as_array_mut() {
            values.push(inherited);
        }
        Ok(())
    }

    fn type_reference(&mut self, at: &NodeRef, lexical: &str, site: usize) -> Result<Value> {
        let name = self.name(at, lexical, site)?;
        if name.namespace == XSD
            && !matches!(
                name.local.as_str(),
                "string"
                    | "boolean"
                    | "integer"
                    | "long"
                    | "int"
                    | "short"
                    | "byte"
                    | "unsignedLong"
                    | "unsignedInt"
                    | "unsignedShort"
                    | "unsignedByte"
                    | "nonNegativeInteger"
                    | "positiveInteger"
                    | "nonPositiveInteger"
                    | "negativeInteger"
                    | "anyType"
                    | "anySimpleType"
                    | "anyAtomicType"
            )
        {
            self.loss(at,"XSD_VALUE_SPACE_PROJECTION",format!("XML {} lexical forms, whitespace rules or value-space constraints require original XSD validation. OpenAPI projection retains portable JSON shape and XML type metadata.",name.local));
        }
        self.reference(Kind::Type, &name)
    }

    fn documentation(&self, at: &NodeRef, schema: &mut Value) -> Result<()> {
        let mut text = Vec::new();
        for annotation in self.catalog.children(at, XSD, "annotation")? {
            for documentation in self.catalog.children(&annotation, XSD, "documentation")? {
                let mut pending = vec![documentation];
                while let Some(node) = pending.pop() {
                    let value = self.catalog.node(&node)?;
                    if !value.text.trim().is_empty() {
                        text.push(value.text.trim().to_owned());
                    }
                    pending.extend(value.children.iter().rev().map(|id| NodeRef {
                        uri: node.uri.clone(),
                        id: *id,
                    }));
                }
            }
        }
        if !text.is_empty() {
            schema["description"] = json!(text.join("\n\n"));
        }
        Ok(())
    }

    fn loss(&mut self, at: &NodeRef, code: &str, message: impl Into<String>) {
        if let Ok(node) = self.catalog.node(at) {
            self.trace.record(
                self.sources,
                &node.location.address.uri,
                &node.location.address.pointer,
                &[],
                code,
                Classification::Omitted,
                message,
                None,
            );
        }
    }
}

pub(super) fn reference(catalog: &Catalog, kind: Kind, name: &Name) -> Result<Value> {
    if kind == Kind::Type && name.namespace == XSD {
        return builtin(&name.local);
    }
    catalog.declaration(kind, name)?;
    Ok(json!({"$ref":format!("#{}",child("/components/schemas",&component_name(kind,name)))}))
}

fn component_name(kind: Kind, name: &Name) -> String {
    let local: String = name
        .local
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .take(96)
        .collect();
    let digest = hash(format!("{kind:?}:{}#{}", name.namespace, name.local).as_bytes());
    format!("Xsd{kind:?}_{local}_{}", &digest[..16])
}

fn builtin(name: &str) -> Result<Value> {
    let mut schema = match name {
        "anyType" | "anySimpleType" | "anyAtomicType" => json!({}),
        "string" | "normalizedString" | "token" | "language" | "Name" | "NCName" | "NMTOKEN"
        | "NMTOKENS" | "ID" | "IDREF" | "IDREFS" | "ENTITY" | "ENTITIES" | "QName" | "NOTATION"
        | "duration" | "yearMonthDuration" | "dayTimeDuration" | "date" | "dateTime"
        | "dateTimeStamp" | "time" | "gYear" | "gMonth" | "gDay" | "gYearMonth" | "gMonthDay" => {
            json!({"type":"string"})
        }
        "boolean" => json!({"type":"boolean"}),
        "decimal" => json!({"type":"number"}),
        "float" | "double" => json!({"type":"number","format":name}),
        "integer" => json!({"type":"integer"}),
        "long" => json!({"type":"integer","format":"int64","minimum":i64::MIN,"maximum":i64::MAX}),
        "int" => json!({"type":"integer","format":"int32","minimum":i32::MIN,"maximum":i32::MAX}),
        "short" => json!({"type":"integer","minimum":i16::MIN,"maximum":i16::MAX}),
        "byte" => json!({"type":"integer","minimum":i8::MIN,"maximum":i8::MAX}),
        "unsignedLong" => json!({"type":"integer","minimum":0,"maximum":u64::MAX}),
        "unsignedInt" => json!({"type":"integer","minimum":0,"maximum":u32::MAX}),
        "unsignedShort" => json!({"type":"integer","minimum":0,"maximum":u16::MAX}),
        "unsignedByte" => json!({"type":"integer","minimum":0,"maximum":u8::MAX}),
        "nonNegativeInteger" => json!({"type":"integer","minimum":0}),
        "positiveInteger" => json!({"type":"integer","minimum":1}),
        "nonPositiveInteger" => json!({"type":"integer","maximum":0}),
        "negativeInteger" => json!({"type":"integer","maximum":-1}),
        "anyURI" => json!({"type":"string","format":"uri-reference"}),
        "base64Binary" => json!({"type":"string","contentEncoding":"base64"}),
        "hexBinary" => json!({"type":"string","pattern":"^(?:[0-9A-Fa-f]{2})*$"}),
        _ => {
            return Err(ConversionError::new(
                "XSD_BUILTIN",
                format!("Unknown XML Schema builtin type {name}"),
            ));
        }
    };
    schema["x-xsd-type"] = json!(name);
    Ok(schema)
}

fn literal(schema: &Value, value: &str) -> Option<Value> {
    match schema["type"].as_str()? {
        "string" => Some(json!(value)),
        "boolean" => match value {
            "true" | "1" => Some(json!(true)),
            "false" | "0" => Some(json!(false)),
            _ => None,
        },
        "integer" | "number" => exact_number(value).map(Value::Number),
        _ => None,
    }
}

fn exact_number(value: &str) -> Option<serde_json::Number> {
    octri_parser::numbers::parse_decimal(value.trim())
}
