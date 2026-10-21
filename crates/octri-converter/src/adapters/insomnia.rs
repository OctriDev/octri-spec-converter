//! Insomnia v4 resources and v5 nested collections with explicit environment selection.
use super::{
    common::{object, unknown_fields},
    http::{self, Auth, Header, Observation, Payload},
    variables,
};
use crate::{Classification, Config, ConversionError, Result};
use crate::{
    source::{Sources, child},
    trace::Trace,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

struct Context<'a> {
    sources: &'a Sources,
    config: &'a Config,
    trace: &'a mut Trace,
    requests: Vec<Observation>,
    variables: BTreeMap<String, String>,
}

pub(crate) fn convert(sources: &mut Sources, config: &Config, trace: &mut Trace) -> Result<Value> {
    let uri = sources.root.clone();
    let document = sources.parse(&uri, false)?;
    let mut context = Context {
        sources,
        config,
        trace,
        requests: Vec::new(),
        variables: BTreeMap::new(),
    };
    let title = if document["_type"] == "export" {
        if document["__export_format"] != 4 {
            return Err(ConversionError::new(
                "INSOMNIA_VERSION",
                "Expected Insomnia v4 JSON export",
            ));
        }
        let resources = document["resources"].as_array().ok_or_else(|| {
            ConversionError::new("INSOMNIA_RESOURCES", "Export needs resources array")
        })?;
        context.v4(resources)?;
        resources
            .iter()
            .find(|value| value["_type"] == "workspace")
            .and_then(|value| value["name"].as_str())
            .unwrap_or("Converted Insomnia collection")
    } else {
        let kind = document["type"].as_str().unwrap_or("");
        if !matches!(
            kind,
            "collection.insomnia.rest/5.0" | "spec.insomnia.rest/5.0"
        ) {
            return Err(ConversionError::new(
                "INSOMNIA_VERSION",
                "Expected Insomnia v5 collection or spec export",
            ));
        }
        if let Some(spec) = document.get("spec") {
            context.record("/spec","EMBEDDED_SPEC_NOT_SELECTED",Classification::Omitted,"Embedded specification remains in original export. This conversion uses collection requests; import embedded OpenAPI directly to preserve its authored contracts.",Some(spec));
        }
        variables::flatten(
            &document["environments"]["data"],
            "",
            &mut context.variables,
        );
        if let Some(environments) = document
            .pointer("/environments/subEnvironments")
            .and_then(Value::as_array)
        {
            context.select_environment(environments)?;
        }
        context.variables.extend(config.variables.clone());
        context.v5(
            document.get("collection"),
            "/collection",
            &[],
            &Auth::Unknown,
            &[],
            0,
        )?;
        for key in ["testSuites", "cookieJar", "certificates"] {
            if let Some(value) = document.get(key) {
                context.record(&child("",key),"INSOMNIA_RUNTIME_RESOURCE",Classification::Omitted,"Runtime resource retained in original export; no execution or file access occurs",Some(value));
            }
        }
        document["name"]
            .as_str()
            .unwrap_or("Converted Insomnia collection")
    };
    let mut builder = http::Builder::new(title, sources, config, context.trace);
    for request in context.requests {
        builder.add(request)?;
    }
    Ok(builder.finish())
}

impl Context<'_> {
    fn select_environment(&mut self, environments: &[Value]) -> Result<()> {
        if let Some(id) = &self.config.environment_id {
            let selected = environments
                .iter()
                .find(|value| {
                    value["_id"]
                        .as_str()
                        .or_else(|| value.pointer("/meta/id").and_then(Value::as_str))
                        == Some(id)
                })
                .ok_or_else(|| {
                    ConversionError::new(
                        "INSOMNIA_ENVIRONMENT",
                        format!("No exported environment {id}"),
                    )
                })?;
            variables::flatten(&selected["data"], "", &mut self.variables);
            for entry in selected
                .get("kvPairData")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if entry["enabled"] == false {
                    continue;
                }
                if let (Some(name), Some(value)) = (entry["name"].as_str(), entry["value"].as_str())
                {
                    self.variables.insert(name.into(), value.into());
                }
            }
        } else if !environments.is_empty() {
            self.record("/environments","ENVIRONMENT_NOT_SELECTED",Classification::Inferred,"Sub-environments exist. Base environment used; choose environmentId or confirm this selection.",None);
        }
        Ok(())
    }

    fn v4(&mut self, resources: &[Value]) -> Result<()> {
        let mut ids = BTreeMap::new();
        let mut environments = Vec::new();
        for (index, value) in resources.iter().enumerate() {
            let id = value["_id"]
                .as_str()
                .ok_or_else(|| ConversionError::new("INSOMNIA_RESOURCE", "Resource needs _id"))?;
            if ids.insert(id, (index, value)).is_some() {
                return Err(ConversionError::new(
                    "INSOMNIA_RESOURCE",
                    "Duplicate exported resource ID",
                ));
            }
            if value["_type"] == "environment" {
                environments.push(value.clone());
            }
        }
        for environment in &environments {
            let parent = environment["parentId"].as_str().unwrap_or("");
            if ids
                .get(parent)
                .is_some_and(|(_, value)| value["_type"] == "workspace")
            {
                variables::flatten(&environment["data"], "", &mut self.variables);
            }
        }
        let children = environments
            .iter()
            .filter(|value| {
                value["parentId"].as_str().is_some_and(|parent| {
                    environments
                        .iter()
                        .any(|environment| environment["_id"] == parent)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        self.select_environment(&children)?;
        self.variables.extend(self.config.variables.clone());
        for (index, request) in resources.iter().enumerate() {
            let pointer = child("/resources", &index.to_string());
            match request["_type"].as_str(){
                Some("request")=>{
                    let mut ancestors=Vec::new();let mut parent=request["parentId"].as_str();let mut seen=BTreeSet::new();
                    while let Some(id)=parent{
                        if !seen.insert(id)||seen.len()>self.config.limits.max_depth{return Err(ConversionError::new("INSOMNIA_PARENT_CYCLE","Resource parent graph is cyclic or too deep"));}
                        let (_,value)=ids.get(id).ok_or_else(||ConversionError::new("INSOMNIA_PARENT",format!("Missing parent resource {id}")))?;ancestors.push(*value);parent=value["parentId"].as_str();
                    }
                    ancestors.reverse();let previous=self.variables.clone();let mut tags=Vec::new();let mut auth=Auth::Unknown;let mut headers=Vec::new();
                    for ancestor in ancestors{
                        if matches!(ancestor["_type"].as_str(),Some("request_group"|"folder")){
                            if let Some(name)=ancestor["name"].as_str(){tags.push(name.into());}
                            variables::flatten(&ancestor["environment"],"",&mut self.variables);self.variables.extend(self.config.variables.clone());
                            auth=self.auth(ancestor.get("authentication"),&pointer,&auth)?;
                            headers=self.headers(ancestor.get("headers"),&pointer,headers)?;
                        }
                    }
                    let request=self.request(request,&pointer,&tags,&auth,&headers)?;self.requests.push(request);self.variables=previous;
                },
                Some("workspace"|"environment"|"request_group"|"folder")=>{},
                _=>self.record(&pointer,"INSOMNIA_RUNTIME_RESOURCE",Classification::Omitted,"Non-HTTP resource retained in original export. Supply its original protocol schema and HTTP mappings to convert it.",Some(request)),
            }
        }
        Ok(())
    }

    fn v5(
        &mut self,
        value: Option<&Value>,
        pointer: &str,
        tags: &[String],
        auth: &Auth,
        headers: &[Header],
        depth: usize,
    ) -> Result<()> {
        if depth > self.config.limits.max_depth {
            return Err(ConversionError::new(
                "INSOMNIA_DEPTH",
                "Collection nesting exceeds configured depth",
            ));
        }
        let Some(value) = value else {
            return Ok(());
        };
        let children = value.as_array().ok_or_else(|| {
            ConversionError::new(
                "INSOMNIA_COLLECTION",
                "Collection children must be an array",
            )
        })?;
        for (index, item) in children.iter().enumerate() {
            let at = child(pointer, &index.to_string());
            if item.get("children").is_some() {
                let previous = self.variables.clone();
                variables::flatten(&item["environment"], "", &mut self.variables);
                self.variables.extend(self.config.variables.clone());
                let mut tags = tags.to_vec();
                if let Some(name) = item["name"].as_str() {
                    tags.push(name.into());
                }
                let auth = self.auth(
                    item.get("authentication"),
                    &child(&at, "authentication"),
                    auth,
                )?;
                let headers = self.headers(
                    item.get("headers"),
                    &child(&at, "headers"),
                    headers.to_vec(),
                )?;
                self.scripts(item, &at);
                self.v5(
                    item.get("children"),
                    &child(&at, "children"),
                    &tags,
                    &auth,
                    &headers,
                    depth + 1,
                )?;
                self.variables = previous;
            } else if item.get("method").is_some() {
                let request = self.request(item, &at, tags, auth, headers)?;
                self.requests.push(request);
            } else {
                self.record(&at,"INSOMNIA_NON_HTTP_RESOURCE",Classification::Blocked,"Collection item is not a declared HTTP request. Supply its protocol source and an explicit HTTP mapping.",Some(item));
            }
        }
        Ok(())
    }

    fn request(
        &mut self,
        value: &Value,
        pointer: &str,
        tags: &[String],
        inherited_auth: &Auth,
        inherited_headers: &[Header],
    ) -> Result<Observation> {
        object(value, "Insomnia request")?;
        self.scripts(value, pointer);
        let method = value["method"]
            .as_str()
            .ok_or_else(|| ConversionError::new("INSOMNIA_METHOD", "Request needs HTTP method"))?
            .to_owned();
        let mut url = self.expand(
            value["url"]
                .as_str()
                .ok_or_else(|| ConversionError::new("INSOMNIA_URL", "Request needs URL"))?,
            &child(pointer, "url"),
        )?;
        if let Some(parameters) = value.get("parameters").and_then(Value::as_array) {
            let mut pairs = Vec::new();
            for entry in parameters {
                if entry["disabled"] == true {
                    continue;
                }
                let name = entry["name"].as_str().ok_or_else(|| {
                    ConversionError::new("INSOMNIA_QUERY", "Query entry needs name")
                })?;
                pairs.push((
                    self.expand(name, pointer)?,
                    self.expand(entry["value"].as_str().unwrap_or(""), pointer)?,
                ));
            }
            if !pairs.is_empty() {
                let mut parsed = url::Url::parse(&url)
                    .map_err(|error| ConversionError::new("INSOMNIA_URL", error.to_string()))?;
                parsed.query_pairs_mut().extend_pairs(pairs);
                url = parsed.to_string();
            }
        }
        let headers = self.headers(
            value.get("headers"),
            &child(pointer, "headers"),
            inherited_headers.to_vec(),
        )?;
        let auth = self.auth(
            value.get("authentication"),
            &child(pointer, "authentication"),
            inherited_auth,
        )?;
        let body = self.body(value.get("body"), &child(pointer, "body"))?;
        let mut path_variables = BTreeMap::new();
        for variable in value
            .get("pathParameters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let (Some(name), Some(example)) =
                (variable["name"].as_str(), variable["value"].as_str())
            {
                path_variables.insert(name.into(), self.expand(example, pointer)?);
            }
        }
        unknown_fields(
            value,
            &[
                "_id",
                "_type",
                "parentId",
                "name",
                "description",
                "url",
                "method",
                "body",
                "parameters",
                "headers",
                "authentication",
                "pathParameters",
                "scripts",
                "preRequestScript",
                "afterResponseScript",
                "settings",
                "meta",
                "created",
                "modified",
                "metaSortKey",
                "isPrivate",
                "settingStoreCookies",
                "settingSendCookies",
                "settingDisableRenderRequestBody",
                "settingEncodeUrl",
                "settingRebuildPath",
                "settingFollowRedirects",
                "disableUserAgentHeader",
                "konnectRouteKey",
                "konnectManagedHeaderNames",
            ],
            self.sources,
            &self.sources.root,
            pointer,
            self.trace,
        );
        Ok(Observation {
            pointer: pointer.into(),
            name: value["name"].as_str().unwrap_or("Imported request").into(),
            description: value["description"]
                .as_str()
                .or_else(|| value.pointer("/meta/description").and_then(Value::as_str))
                .map(str::to_owned),
            method,
            url,
            headers,
            path_variables,
            body,
            responses: Vec::new(),
            tags: tags.to_vec(),
            auth,
        })
    }

    fn body(&mut self, value: Option<&Value>, pointer: &str) -> Result<Option<Payload>> {
        let Some(value) =
            value.filter(|value| value.as_object().is_some_and(|map| !map.is_empty()))
        else {
            return Ok(None);
        };
        let media = value["mimeType"].as_str().filter(|media| !media.is_empty());
        if let Some(text) = value["text"].as_str() {
            let text = self.expand(text, pointer)?;
            return http::example_payload(&text, media, pointer, self.sources);
        }
        if value.get("fileName").is_some() {
            self.record(
                pointer,
                "FILE_CONTENT_NOT_READ",
                Classification::Transformed,
                "Binary upload declared; file path remains inert",
                Some(value),
            );
            return Ok(Some(Payload {
                media: media.unwrap_or("application/octet-stream").into(),
                schema: json!({"type":"string","format":"binary"}),
                example: None,
                encoding: None,
                pointer: pointer.into(),
                inferred: false,
            }));
        }
        if let Some(fields) = value["params"].as_array() {
            let mut properties = serde_json::Map::new();
            let mut examples = serde_json::Map::new();
            for field in fields {
                if field["disabled"] == true {
                    continue;
                }
                let name = field["name"].as_str().ok_or_else(|| {
                    ConversionError::new("INSOMNIA_BODY", "Form entry needs name")
                })?;
                if properties.contains_key(name) {
                    return Err(ConversionError::new(
                        "INSOMNIA_BODY",
                        "Repeated form fields need an authored array mapping",
                    ));
                }
                if field["type"] == "file" || field.get("fileName").is_some() {
                    properties.insert(name.into(), json!({"type":"string","format":"binary"}));
                } else {
                    properties.insert(name.into(), json!({"type":"string"}));
                    examples.insert(
                        name.into(),
                        json!(self.expand(field["value"].as_str().unwrap_or(""), pointer)?),
                    );
                }
            }
            return Ok(Some(Payload {
                media: media.unwrap_or("application/x-www-form-urlencoded").into(),
                schema: json!({"type":"object","properties":properties}),
                example: Some(Value::Object(examples)),
                encoding: None,
                pointer: pointer.into(),
                inferred: true,
            }));
        }
        self.record(
            pointer,
            "INSOMNIA_BODY_UNMAPPED",
            Classification::Blocked,
            "Body shape requires an explicit payload mapping",
            Some(value),
        );
        Ok(None)
    }

    fn headers(
        &mut self,
        value: Option<&Value>,
        pointer: &str,
        mut inherited: Vec<Header>,
    ) -> Result<Vec<Header>> {
        let Some(value) = value else {
            return Ok(inherited);
        };
        let headers = value
            .as_array()
            .ok_or_else(|| ConversionError::new("INSOMNIA_HEADER", "Headers must be an array"))?;
        for (index, header) in headers.iter().enumerate() {
            if header["disabled"] == true {
                continue;
            }
            let at = child(pointer, &index.to_string());
            let name = header["name"]
                .as_str()
                .ok_or_else(|| ConversionError::new("INSOMNIA_HEADER", "Header needs name"))?;
            let name = self.expand(name, &at)?;
            let value = self.expand(header["value"].as_str().unwrap_or(""), &at)?;
            inherited.retain(|header| !header.name.eq_ignore_ascii_case(&name));
            inherited.push(Header {
                name,
                value,
                pointer: at,
            });
        }
        Ok(inherited)
    }

    fn auth(&mut self, value: Option<&Value>, pointer: &str, inherited: &Auth) -> Result<Auth> {
        let Some(value) = value else {
            return Ok(inherited.clone());
        };
        let Some(kind) = value["type"].as_str() else {
            return Ok(inherited.clone());
        };
        if value["disabled"] == true || kind == "none" {
            return Ok(Auth::None);
        }
        let mut required_scopes = Vec::new();
        let scheme = match kind {
            "basic" | "bearer" | "digest" => json!({"type":"http","scheme":kind}),
            "apikey" => {
                let name = self.expand(
                    value["key"].as_str().ok_or_else(|| {
                        ConversionError::new("INSOMNIA_AUTH", "API key auth needs key")
                    })?,
                    pointer,
                )?;
                let location = value["addTo"].as_str().unwrap_or("header");
                if !matches!(location, "header" | "query") {
                    return Err(ConversionError::new(
                        "INSOMNIA_AUTH",
                        "API key location must be header or query",
                    ));
                }
                json!({"type":"apiKey","name":name,"in":location})
            }
            "oauth2" => {
                let flow = match value["grantType"].as_str() {
                    Some("authorization_code") => "authorizationCode",
                    Some("client_credentials") => "clientCredentials",
                    Some("password") => "password",
                    Some("implicit") => "implicit",
                    _ => {
                        self.record(pointer,"AUTH_UNREPRESENTABLE",Classification::Blocked,"OAuth grant has no equivalent OpenAPI flow; provide an explicit security scheme",Some(value));
                        return Ok(Auth::Unknown);
                    }
                };
                let mut settings = json!({"scopes":{}});
                for (source, target) in [
                    ("authorizationUrl", "authorizationUrl"),
                    ("accessTokenUrl", "tokenUrl"),
                ] {
                    if let Some(url) = value[source].as_str() {
                        settings[target] = json!(self.expand(url, pointer)?);
                    }
                }
                if let Some(scopes) = value["scope"].as_str() {
                    let scopes = self.expand(scopes, &child(pointer, "scope"))?;
                    required_scopes = http::oauth_scopes(&scopes);
                    for scope in &required_scopes {
                        settings["scopes"][scope] = json!("");
                    }
                }
                json!({"type":"oauth2","flows":{flow:settings}})
            }
            _ => {
                self.record(
                    pointer,
                    "AUTH_UNREPRESENTABLE",
                    Classification::Blocked,
                    format!("{kind} authentication needs an explicit OpenAPI scheme"),
                    Some(value),
                );
                return Ok(Auth::Unknown);
            }
        };
        Ok(Auth::scoped_scheme(kind, scheme, required_scopes))
    }

    fn scripts(&mut self, value: &Value, pointer: &str) {
        for key in ["scripts", "preRequestScript", "afterResponseScript"] {
            if let Some(script) = value
                .get(key)
                .filter(|value| !value.is_null() && value.as_str() != Some(""))
            {
                self.record(&child(pointer,key),"SCRIPT_NOT_EXECUTED",Classification::Omitted,"Insomnia scripts retained in original source; dynamic mutations require explicit literal values",Some(script));
            }
        }
    }
    fn expand(&self, raw: &str, _pointer: &str) -> Result<String> {
        variables::expand(raw, &self.variables, self.config.limits.max_document_bytes)
    }
    fn record(
        &mut self,
        pointer: &str,
        code: &str,
        class: Classification,
        message: impl Into<String>,
        evidence: Option<&Value>,
    ) {
        self.trace.record(
            self.sources,
            &self.sources.root,
            pointer,
            &[],
            code,
            class,
            message,
            evidence,
        );
    }
}
