//! RAML HTTP descriptions: source loading, templates, schemas and transport.
mod body;
mod formats;
mod includes;
mod libraries;
mod parameters;
mod schema_fragments;
mod schema_use;
mod serialization;
mod template_value;
mod templates;
mod types;
mod version;
mod xml_schemas;

use super::common::{method, operation_id, unknown_fields};
use super::references::Schemas;
use crate::{Classification, Config, ConversionError, Result};
use crate::{
    source::{Sources, child},
    trace::Trace,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

struct Context<'a> {
    version: version::Version,
    sources: &'a mut Sources,
    config: &'a Config,
    trace: &'a mut Trace,
    names: BTreeSet<String>,
    type_declarations: Map<String, Value>,
    external_names: BTreeMap<String, schema_use::SchemaKind>,
    legacy_schemas: Map<String, Value>,
    media: Vec<String>,
    document: Value,
    schemas: Schemas,
    xml_schemas: xml_schemas::XmlSchemas,
}

pub(crate) fn convert(sources: &mut Sources, config: &Config, trace: &mut Trace) -> Result<Value> {
    let raml_version = version::Version::read(sources)?;
    let mut root = includes::expand(sources)?;
    raml_version.normalize(&mut root, sources)?;
    if raml_version == version::Version::Raml08 {
        for group in ["traits", "resourceTypes", "securitySchemes", "schemas"] {
            for name in root
                .get(group)
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(Map::keys)
            {
                trace.record(
                    sources,
                    &sources.root,
                    &child(&child("", group), name),
                    &[],
                    "RAML_LEGACY_DECLARATION",
                    Classification::Transformed,
                    "RAML 0.8 declaration array normalized for template and transport processing; physical source remains unchanged",
                    None,
                );
            }
        }
    }
    let root = if raml_version == version::Version::Raml10 {
        libraries::apply(root, sources)?
    } else {
        root
    };
    let title = root["title"]
        .as_str()
        .ok_or_else(|| ConversionError::new("RAML_TITLE", "RAML API requires a title"))?;
    let version = config
        .api_version
        .as_deref()
        .or_else(|| root["version"].as_str())
        .unwrap_or("1.0.0");
    let mut document = json!({"openapi":"3.1.2","info":{"title":config.title.as_deref().unwrap_or(title),"version":version},"paths":{},"components":{"schemas":{},"securitySchemes":{}}});
    if let Some(description) = root.get("description") {
        document["info"]["description"] = description.clone();
    }
    let names = root
        .get("types")
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    let media = match root.get("mediaType") {
        Some(Value::String(media)) => vec![media.clone()],
        Some(Value::Array(media)) => media
            .iter()
            .map(|media| {
                media.as_str().map(str::to_owned).ok_or_else(|| {
                    ConversionError::new("RAML_MEDIA", "Media types must be strings")
                })
            })
            .collect::<Result<Vec<_>>>()?,
        None => Vec::new(),
        _ => {
            return Err(ConversionError::new(
                "RAML_MEDIA",
                "mediaType must be a string or array",
            ));
        }
    };
    let null = Value::Null;
    let mut templates = templates::Templates::new(
        root.get("traits").unwrap_or(&null),
        root.get("resourceTypes").unwrap_or(&null),
        sources,
        &config.limits,
    );
    let external_names = schema_use::external_names(&root, sources);
    let mut context = Context {
        version: raml_version,
        sources,
        config,
        trace,
        names,
        type_declarations: root
            .get("types")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        external_names,
        legacy_schemas: root
            .get("schemas")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        media,
        document,
        schemas: Schemas::default(),
        xml_schemas: xml_schemas::XmlSchemas::default(),
    };
    if root.get("version").is_none() && config.api_version.is_none() {
        context.record(
            "/version",
            &["/info/version".into()],
            "API_VERSION_INFERRED",
            Classification::Inferred,
            "RAML has no API version. Review proposed OpenAPI info.version.",
            None,
        );
    }
    context.document["servers"] = context.servers(&root)?;
    for (name, value) in root
        .get("types")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let schema = context.schema(value, &child("/types", name))?;
        context.document["components"]["schemas"][name] = schema;
    }
    context.security_schemes(root.get("securitySchemes"))?;
    if let Some(security) = root.get("securedBy") {
        context.document["security"] = context.security(security, "/securedBy")?;
    }
    context.resources(&root, "", "", &BTreeMap::new(), &mut templates, 0)?;
    let mut known = vec![
        "title",
        "description",
        "version",
        "baseUri",
        "baseUriParameters",
        "protocols",
        "mediaType",
        "types",
        "schemas",
        "securitySchemes",
        "securedBy",
        "traits",
        "resourceTypes",
        "uses",
        "annotationTypes",
        "documentation",
    ];
    known.extend(
        root.as_object()
            .into_iter()
            .flat_map(Map::keys)
            .filter(|name| name.starts_with('/') || (name.starts_with('(') && name.ends_with(')')))
            .map(String::as_str),
    );
    unknown_fields(
        &root,
        &known,
        context.sources,
        &context.sources.root,
        "",
        context.trace,
    );
    for key in ["documentation", "annotationTypes", "schemas"] {
        if key == "schemas" && raml_version == version::Version::Raml08 {
            continue;
        }
        if let Some(value) = root.get(key) {
            context.record(&child("",key),&[],"RAML_AUXILIARY_DECLARATION",if key=="uses"{Classification::Blocked}else{Classification::Omitted},format!("{key} declaration retained in original sources; explicit processing is required before claiming full fidelity"),Some(value));
        }
    }
    let mut document = context.document;
    let mut definitions = document["components"]["schemas"]
        .as_object_mut()
        .map(std::mem::take)
        .ok_or_else(|| {
            ConversionError::new(
                "SCHEMA_REGISTRY",
                "Generated RAML schema registry is missing",
            )
        })?;
    for (name, schema) in context.xml_schemas.definitions {
        if definitions.insert(name, schema).is_some() {
            return Err(ConversionError::new(
                "XSD_COMPONENT_COLLISION",
                "XML projection conflicts with an authored RAML type name",
            ));
        }
    }
    document["components"]["schemas"] =
        Value::Object(context.schemas.finish(sources, definitions)?);
    Ok(document)
}

impl Context<'_> {
    fn schema(&mut self, value: &Value, pointer: &str) -> Result<Value> {
        self.schema_at(value, pointer, schema_use::SchemaUse::Declaration)
    }

    fn schema_at(
        &mut self,
        value: &Value,
        pointer: &str,
        usage: schema_use::SchemaUse,
    ) -> Result<Value> {
        types::Types {
            names: &self.names,
            declarations: &self.type_declarations,
            external_names: &self.external_names,
            sources: self.sources,
            trace: self.trace,
            schemas: &mut self.schemas,
            xml_schemas: &mut self.xml_schemas,
        }
        .schema(value, pointer, 0, usage)
    }

    fn servers(&mut self, root: &Value) -> Result<Value> {
        if let Some(parameters) = root.get("baseUriParameters") {
            let parameters = parameters.as_object().ok_or_else(|| {
                ConversionError::new("RAML_PARAMETER", "baseUriParameters must be an object")
            })?;
            for (name, declaration) in parameters {
                self.parameter_schema(declaration, &child("/baseUriParameters", name))?;
            }
        }
        if let Some(url) = &self.config.base_url {
            return Ok(json!([{"url":url}]));
        }
        let Some(raw) = root["baseUri"].as_str() else {
            self.record(
                "/baseUri",
                &["/servers".into()],
                "BASE_URI_UNDECLARED",
                Classification::Inferred,
                "RAML baseUri is absent. Review the relative server before publishing.",
                None,
            );
            return Ok(json!([{"url":"/"}]));
        };
        let version = root["version"].as_str().unwrap_or("");
        let mut url = raw.replace("{version}", version);
        let mut variables = Map::new();
        for part in raw.split('{').skip(1) {
            let (name, _) = part
                .split_once('}')
                .ok_or_else(|| ConversionError::new("RAML_BASE_URI", "Unclosed URI template"))?;
            if name == "version" {
                continue;
            }
            let declaration = &root["baseUriParameters"][name];
            let default = self
                .config
                .variables
                .get(name)
                .cloned()
                .or_else(|| declaration["default"].as_str().map(str::to_owned));
            if let Some(default) = default {
                url = url.replace(&format!("{{{name}}}"), &default);
            } else {
                self.record(&child("/baseUriParameters",name),&["/servers".into()],"SERVER_VARIABLE_DEFAULT",Classification::Blocked,"OpenAPI server variables require an explicit default. Supply variables or baseUrl.",Some(declaration));
                variables.insert(name.into(), json!({"default":""}));
            }
        }
        let schemes = root["protocols"]
            .as_array()
            .map(|protocols| {
                protocols
                    .iter()
                    .map(|protocol| {
                        protocol
                            .as_str()
                            .map(str::to_ascii_lowercase)
                            .filter(|protocol| matches!(protocol.as_str(), "http" | "https"))
                            .ok_or_else(|| {
                                ConversionError::new(
                                    "RAML_PROTOCOL",
                                    "Protocol must be HTTP or HTTPS",
                                )
                            })
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        let urls = if let Some(schemes) = schemes {
            if schemes.is_empty() {
                return Err(ConversionError::new(
                    "RAML_PROTOCOL",
                    "Protocol list cannot be empty",
                ));
            }
            let suffix = url
                .split_once("://")
                .map(|(_, suffix)| suffix)
                .ok_or_else(|| {
                    ConversionError::new(
                        "RAML_BASE_URI",
                        "Protocol override requires an absolute base URI",
                    )
                })?;
            schemes
                .iter()
                .map(|scheme| format!("{scheme}://{suffix}"))
                .collect::<Vec<_>>()
        } else {
            vec![url]
        };
        Ok(json!(
            urls.into_iter()
                .map(|url| if variables.is_empty() {
                    json!({"url":url})
                } else {
                    json!({"url":url,"variables":variables})
                })
                .collect::<Vec<_>>()
        ))
    }

    fn resources(
        &mut self,
        parent: &Value,
        prefix: &str,
        pointer: &str,
        inherited: &BTreeMap<String, template_value::LocatedValue>,
        templates: &mut templates::Templates<'_>,
        depth: usize,
    ) -> Result<()> {
        if depth > self.config.limits.max_depth {
            return Err(ConversionError::new(
                "RAML_RESOURCE_DEPTH",
                "Resource nesting exceeds configured depth",
            ));
        }
        for (relative, raw) in parent
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(name, _)| name.starts_with('/'))
        {
            let path = format!("{prefix}{relative}");
            let at = child(pointer, relative);
            let resource_node = templates.resource(
                template_value::LocatedValue::read(raw, &at, self.sources),
                &path,
            )?;
            let resource_node =
                template_value::optional_methods(resource_node, &self.config.limits)?;
            resource_node.install_origins(&at, self.sources);
            let resource = resource_node.to_value();
            let mut uri_parameters = inherited.clone();
            if let Some(parameters) = resource_node.get("uriParameters")
                && let template_value::Kind::Object(parameters) = &parameters.kind
            {
                uri_parameters.extend(
                    parameters
                        .iter()
                        .map(|(name, node)| (name.clone(), node.clone())),
                );
            }
            for (verb, _) in resource
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(verb, _)| method(verb))
            {
                let operation_pointer = child(&at, verb);
                let raw_node = resource_node.get(verb).ok_or_else(|| {
                    ConversionError::new("RAML_TEMPLATE", "Expanded operation is missing")
                })?;
                let operation_node = templates.operation(raw_node, &resource_node, &path, verb)?;
                operation_node.install_origins(&operation_pointer, self.sources);
                let operation = operation_node.to_value();
                let converted = self.operation(
                    &operation,
                    &resource,
                    &path,
                    verb,
                    &operation_pointer,
                    &uri_parameters,
                )?;
                if self.document["paths"][&path].get(verb).is_some() {
                    return Err(ConversionError::new(
                        "RAML_PATH_COLLISION",
                        format!("Multiple resources produce {verb} {path}"),
                    ));
                }
                self.document["paths"][&path][verb] = converted;
            }
            self.resources(&resource, &path, &at, &uri_parameters, templates, depth + 1)?;
        }
        Ok(())
    }

    fn operation(
        &mut self,
        value: &Value,
        resource: &Value,
        path: &str,
        verb: &str,
        pointer: &str,
        uri_parameters: &BTreeMap<String, template_value::LocatedValue>,
    ) -> Result<Value> {
        let mut result = json!({"operationId":operation_id(verb,path),"responses":{}});
        for (source, target) in [("displayName", "summary"), ("description", "description")] {
            if let Some(value) = value.get(source) {
                result[target] = value.clone();
            }
        }
        let mut parameters = Vec::new();
        for part in path.split('{').skip(1) {
            let (name, _) = part.split_once('}').ok_or_else(|| {
                ConversionError::new("RAML_URI_TEMPLATE", "Unclosed path parameter")
            })?;
            if name == "ext" {
                self.record(
                    pointer,
                    &[],
                    "RAML_EXT_PARAMETER",
                    Classification::Blocked,
                    "RAML ext suffix templates need an explicit path/media mapping",
                    Some(&json!(path)),
                );
            }
            let at = child(&child(pointer, "uriParameters"), name);
            let declaration = if let Some(node) = uri_parameters.get(name) {
                node.install_origins(&at, self.sources);
                node.to_value()
            } else {
                Value::Null
            };
            let schema = self.parameter_schema(&declaration, &at)?;
            parameters.push(json!({"in":"path","name":name,"required":true,"schema":schema}));
        }
        for (field, location) in [("queryParameters", "query"), ("headers", "header")] {
            for (name, declaration) in value
                .get(field)
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let at = child(&child(pointer, field), name);
                let (name, optional) = self.parameter_name(name);
                let schema = self.parameter_schema(declaration, &at)?;
                let required = self.parameter_required(declaration, optional)?;
                let mut parameter =
                    json!({"in":location,"name":name,"required":required,"schema":schema});
                if schema["type"] == "array" {
                    parameter["style"] = json!(if location == "query" {
                        "form"
                    } else {
                        "simple"
                    });
                    parameter["explode"] = json!(location == "query");
                }
                if schema["type"] == "object" || schema.get("anyOf").is_some() {
                    self.record(&at,&[],"RAML_PARAMETER_SERIALIZATION",Classification::Inferred,"RAML leaves non-scalar parameter serialization implementation-defined. Review the HTTP serialization mapping.",Some(declaration));
                }
                if self.version == version::Version::Raml08
                    && location == "header"
                    && schema["type"] == "array"
                {
                    self.record(&at,&[],"RAML_PARAMETER_SERIALIZATION",Classification::Inferred,"Repeated header values use OpenAPI simple serialization. Review whether the source runtime sends separate fields or a comma-separated list.",Some(declaration));
                }
                parameters.push(parameter);
            }
        }
        if let Some(query) = value.get("queryString") {
            if self.version == version::Version::Raml08 {
                return Err(ConversionError::new(
                    "RAML_VERSION_FEATURE",
                    "queryString is not a RAML 0.8 method field; declare named queryParameters",
                ));
            }
            if value.get("queryParameters").is_some() {
                return Err(ConversionError::new(
                    "RAML_QUERY",
                    "queryString and queryParameters are mutually exclusive",
                ));
            }
            parameters.push(json!({"name":"queryString","in":"querystring","content":{"application/x-www-form-urlencoded":{"schema":self.schema_at(query,&child(pointer,"queryString"),schema_use::SchemaUse::RamlType)?}}}));
        }
        if !parameters.is_empty() {
            result["parameters"] = json!(parameters);
        }
        if let Some(body) = value.get("body") {
            result["requestBody"] = json!({"content":self.body(body,&child(pointer,"body"))?});
        }
        if let Some(responses) = value.get("responses").and_then(Value::as_object) {
            for (status, response) in responses {
                if !super::http::valid_status(status) {
                    return Err(ConversionError::new(
                        "RAML_RESPONSE",
                        format!("Invalid HTTP status {status}"),
                    ));
                }
                let at = child(&child(pointer, "responses"), status);
                let mut converted = json!({"description":response["description"].as_str().unwrap_or("Declared response")});
                if let Some(body) = response.get("body") {
                    converted["content"] = self.body(body, &child(&at, "body"))?;
                }
                if let Some(headers) = response.get("headers").and_then(Value::as_object) {
                    for (name, declaration) in headers {
                        converted["headers"][name] = json!({"schema":self.parameter_schema(declaration,&child(&child(&at,"headers"),name))?});
                        if self.version == version::Version::Raml08 {
                            converted["headers"][name]["required"] =
                                json!(self.parameter_required(declaration, false)?);
                        }
                    }
                }
                result["responses"][status] = converted;
            }
        }
        if result["responses"]
            .as_object()
            .is_none_or(|responses| responses.is_empty())
        {
            result["responses"] =
                json!({"default":{"description":"Response contract not supplied"}});
            self.record(
                pointer,
                &[],
                "RESPONSE_UNDECLARED",
                Classification::Inferred,
                "RAML operation declares no response. Review default response placeholder.",
                None,
            );
        }
        if let Some(security) = value.get("securedBy").or_else(|| resource.get("securedBy")) {
            result["security"] = self.security(security, &child(pointer, "securedBy"))?;
        }
        if let Some(protocols) = value.get("protocols") {
            self.record(
                &child(pointer, "protocols"),
                &[],
                "RAML_METHOD_PROTOCOLS",
                Classification::Blocked,
                "Method-specific protocol restrictions need an explicit operation server mapping",
                Some(protocols),
            );
        }
        unknown_fields(
            value,
            &[
                "displayName",
                "description",
                "queryParameters",
                "queryString",
                "headers",
                "body",
                "responses",
                "securedBy",
                "is",
                "protocols",
            ],
            self.sources,
            &self.sources.root,
            pointer,
            self.trace,
        );
        self.record(
            pointer,
            &[child(&child("/paths", path), verb)],
            "RAML_OPERATION",
            Classification::Transformed,
            "RAML method, constraints, declared responses and inherited security converted",
            Some(value),
        );
        Ok(result)
    }

    fn body(&mut self, value: &Value, pointer: &str) -> Result<Value> {
        let media_map = value
            .as_object()
            .filter(|map| map.keys().any(|key| key.contains('/')));
        let mut content = Map::new();
        if let Some(media_map) = media_map {
            for (media, declaration) in media_map {
                content.insert(
                    media.clone(),
                    self.media(declaration, &child(pointer, media), media)?,
                );
            }
        } else {
            if self.media.is_empty() {
                self.record(
                    pointer,
                    &[],
                    "RAML_MEDIA_REQUIRED",
                    Classification::Blocked,
                    "Body has no explicit or root media type. Supply an authored content mapping.",
                    Some(value),
                );
                return Ok(json!({}));
            }
            for media in self.media.clone() {
                let converted = self.media(value, pointer, &media)?;
                content.insert(media, converted);
            }
        }
        Ok(Value::Object(content))
    }

    fn media(&mut self, declaration: &Value, pointer: &str, media: &str) -> Result<Value> {
        let essence = media
            .split(';')
            .next()
            .unwrap_or(media)
            .trim()
            .to_ascii_lowercase();
        let xml = essence.ends_with("/xml") || essence.ends_with("+xml");
        let schema = if self.version == version::Version::Raml08 {
            let essence = media.split(';').next().unwrap_or(media).trim();
            let is_form = essence.eq_ignore_ascii_case("application/x-www-form-urlencoded")
                || essence.eq_ignore_ascii_case("multipart/form-data");
            if declaration.get("schema").is_some() && is_form {
                return Err(ConversionError::new(
                    "RAML_FORM",
                    "RAML 0.8 form bodies must use formParameters instead of schema",
                ));
            }
            if declaration.get("formParameters").is_some() && !is_form {
                return Err(ConversionError::new(
                    "RAML_FORM",
                    "formParameters requires form-urlencoded or multipart/form-data media type",
                ));
            }
            self.legacy_body_schema(declaration, pointer, xml)?
        } else {
            let essence = media
                .split(';')
                .next()
                .unwrap_or(media)
                .trim()
                .to_ascii_lowercase();
            let json = essence.ends_with("/json") || essence.ends_with("+json");
            self.schema_at(
                declaration,
                pointer,
                schema_use::SchemaUse::Body { json, xml },
            )?
        };
        let mut result = json!({"schema":schema});
        if self.version == version::Version::Raml08 && declaration.get("formParameters").is_some() {
            for (name, property) in result["schema"]["properties"]
                .as_object()
                .cloned()
                .unwrap_or_default()
            {
                if property["type"] == "array" {
                    result["encoding"][name] = json!({"style":"form","explode":true});
                }
            }
        }
        if let Some(example) = declaration.get("example") {
            result["example"] = if self.version == version::Version::Raml08 {
                example.clone()
            } else {
                example.get("value").unwrap_or(example).clone()
            };
        }
        if self.version == version::Version::Raml10
            && let Some(examples) = declaration.get("examples").and_then(Value::as_object)
        {
            for (name, example) in examples {
                let mut converted = json!({"value": example.get("value").unwrap_or(example)});
                if example.get("value").is_some() {
                    for (source, target) in
                        [("displayName", "summary"), ("description", "description")]
                    {
                        if let Some(value) = example.get(source) {
                            converted[target] = value.clone();
                        }
                    }
                }
                result["examples"][name] = converted;
            }
        }
        Ok(result)
    }

    fn security_schemes(&mut self, value: Option<&Value>) -> Result<()> {
        for (name, scheme) in value.and_then(Value::as_object).into_iter().flatten() {
            let at = child("/securitySchemes", name);
            let converted = match scheme["type"].as_str() {
                Some("Basic Authentication") => json!({"type":"http","scheme":"basic"}),
                Some("Digest Authentication") => json!({"type":"http","scheme":"digest"}),
                Some("OAuth 2.0") => {
                    let settings = &scheme["settings"];
                    let grants = settings["authorizationGrants"].as_array().ok_or_else(|| {
                        ConversionError::new(
                            "RAML_OAUTH",
                            "OAuth settings need authorizationGrants array",
                        )
                    })?;
                    let mut flows = Map::new();
                    for grant in grants {
                        let flow = match grant.as_str() {
                            Some("authorization_code") => "authorizationCode",
                            Some("code") if self.version == version::Version::Raml08 => {
                                "authorizationCode"
                            }
                            Some("implicit") => "implicit",
                            Some("token") if self.version == version::Version::Raml08 => "implicit",
                            Some("password") => "password",
                            Some("client_credentials") => "clientCredentials",
                            _ => {
                                self.record(
                                    &at,
                                    &[],
                                    "RAML_OAUTH_GRANT",
                                    Classification::Blocked,
                                    "OAuth extension grant requires an authored security scheme",
                                    Some(grant),
                                );
                                continue;
                            }
                        };
                        let mut data = json!({"scopes":{}});
                        for scope in settings["scopes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                        {
                            data["scopes"][scope] = json!("");
                        }
                        if matches!(flow, "authorizationCode" | "implicit") {
                            data["authorizationUrl"] = settings["authorizationUri"].clone();
                        }
                        if flow != "implicit" {
                            data["tokenUrl"] = settings["accessTokenUri"].clone();
                        }
                        flows.insert(flow.into(), data);
                    }
                    json!({"type":"oauth2","flows":flows})
                }
                _ => {
                    if let Some(authored) = self.config.security_schemes.get(name) {
                        authored.clone()
                    } else {
                        self.record(&at,&[],"RAML_SECURITY_SCHEME",Classification::Blocked,"Security scheme needs an authored securitySchemes mapping; no weaker scheme substituted",Some(scheme));
                        continue;
                    }
                }
            };
            self.document["components"]["securitySchemes"][name] = converted;
            if let Some(behavior) = scheme.get("describedBy") {
                self.record(&child(&at,"describedBy"),&[],"RAML_SECURITY_BEHAVIOR",Classification::Omitted,"Security-scheme transport/error declarations require review alongside the emitted authentication scheme",Some(behavior));
            }
        }
        Ok(())
    }

    fn security(&mut self, value: &Value, pointer: &str) -> Result<Value> {
        let list = value
            .as_array()
            .ok_or_else(|| ConversionError::new("RAML_SECURITY", "securedBy must be an array"))?;
        let mut requirements = Vec::new();
        for item in list {
            if item.is_null() {
                requirements.push(json!({}));
                continue;
            }
            if let Some(name) = item.as_str() {
                requirements.push(json!({name:[]}));
                continue;
            }
            let map = item
                .as_object()
                .filter(|map| map.len() == 1)
                .ok_or_else(|| {
                    ConversionError::new(
                        "RAML_SECURITY",
                        "Security application must name one scheme",
                    )
                })?;
            for (name, settings) in map {
                requirements.push(
                    json!({name:settings.get("scopes").cloned().unwrap_or_else(||json!([]))}),
                );
                for key in settings
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(key, _)| key)
                    .filter(|key| key.as_str() != "scopes")
                {
                    self.record(
                        pointer,
                        &[],
                        "RAML_SECURITY_PARAMETER",
                        Classification::Omitted,
                        format!("Security parameter {key} requires an authored transport mapping"),
                        Some(settings),
                    );
                }
            }
        }
        Ok(json!(requirements))
    }

    fn record(
        &mut self,
        pointer: &str,
        output: &[String],
        code: &str,
        class: Classification,
        message: impl Into<String>,
        evidence: Option<&Value>,
    ) {
        self.trace.record(
            self.sources,
            &self.sources.root,
            pointer,
            output,
            code,
            class,
            message,
            evidence,
        );
    }
}
