//! Structural validation uses pinned, vendored OpenAPI schemas. Semantic rules
//! operate on the source-aware inventory rather than a lossy endpoint model.

use crate::{Address, Diagnostic, Dialect, Inspection, Location, SchemaDialect, Severity};
use crate::{
    pointer,
    session::Session,
    syntax::{self, Context},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

type Compiled = std::result::Result<jsonschema::ValidatorMap, String>;
static SWAGGER: OnceLock<Compiled> = OnceLock::new();
static OPENAPI_30: OnceLock<Compiled> = OnceLock::new();
static OPENAPI_31: OnceLock<Compiled> = OnceLock::new();
static OPENAPI_32: OnceLock<Compiled> = OnceLock::new();

fn validator(dialect: Dialect) -> &'static Compiled {
    let (cell, schema) = match dialect {
        Dialect::Swagger2 => (&SWAGGER, include_str!("../schemas/openapi-2.0.json")),
        Dialect::OpenApi30 => (&OPENAPI_30, include_str!("../schemas/openapi-3.0.json")),
        Dialect::OpenApi31 => (&OPENAPI_31, include_str!("../schemas/openapi-3.1.json")),
        Dialect::OpenApi32 => (&OPENAPI_32, include_str!("../schemas/openapi-3.2.json")),
    };
    cell.get_or_init(|| {
        let value: Value = serde_json::from_str(schema).map_err(|e| e.to_string())?;
        jsonschema::options()
            .offline()
            .build_map(&value)
            .map_err(|e| e.to_string())
    })
}

pub(crate) fn validate(session: &Session<'_>, inspection: &Inspection) -> Vec<Diagnostic> {
    if session.options.validate.mode == crate::ValidationMode::Compatible {
        return compatible(session, inspection);
    }
    strict(session, inspection)
}

/// Validate the authored root shape, then resolved path declarations. External
/// target shapes and audit-only semantics remain available through strict mode.
fn compatible(session: &Session<'_>, inspection: &Inspection) -> Vec<Diagnostic> {
    let mut diagnostics =
        crate::diagnostics::Diagnostics::new(session.options.limits.max_diagnostics);
    let root = Address {
        uri: session.root_uri.clone(),
        pointer: String::new(),
    };
    if session.options.validate.schema {
        match validator(session.dialect) {
            Ok(validators) => match (validators.get("#"), session.value(&root)) {
                (Some(validator), Some(document)) => {
                    for error in validator
                        .iter_errors(document)
                        .take(session.options.limits.max_diagnostics)
                    {
                        diagnostics.push(error_at(
                            session,
                            "OPENAPI_SCHEMA",
                            error.to_string(),
                            Address {
                                uri: root.uri.clone(),
                                pointer: error.instance_path().to_string(),
                            },
                        ));
                    }
                }
                _ => diagnostics.push(error_at(
                    session,
                    "VALIDATOR_INITIALIZATION",
                    "Root validator or document is missing".into(),
                    root.clone(),
                )),
            },
            Err(error) => diagnostics.push(error_at(
                session,
                "VALIDATOR_INITIALIZATION",
                error.clone(),
                root.clone(),
            )),
        }
    }
    if session.options.validate.spec {
        for operation in &inspection.operations {
            if diagnostics.full() {
                break;
            }
            if operation.kind != "path" {
                continue;
            }
            let names = placeholders(&operation.path)
                .into_iter()
                .filter(|name| !name.is_empty())
                .map(|name| name.strip_suffix('+').unwrap_or(&name).to_string())
                .collect::<BTreeSet<_>>();
            let declared = operation
                .parameters
                .iter()
                .filter(|parameter| parameter.get("in").and_then(Value::as_str) == Some("path"))
                .filter_map(|parameter| parameter.get("name").and_then(Value::as_str))
                .collect::<BTreeSet<_>>();
            for name in &names {
                if !declared.contains(name.as_str()) {
                    diagnostics.push(error_at(
                        session,
                        "MISSING_PATH_PARAMETER",
                        format!("Path template parameter {name} is not declared"),
                        operation.location.address.clone(),
                    ));
                }
            }
            for name in declared {
                if !names.contains(name) {
                    diagnostics.push(error_at(
                        session,
                        "UNUSED_PATH_PARAMETER",
                        format!("Path parameter {name} is absent from template"),
                        operation.location.address.clone(),
                    ));
                }
            }
        }
    }
    dialect_warnings(session, &mut diagnostics);
    diagnostics.finish()
}

fn strict(session: &Session<'_>, inspection: &Inspection) -> Vec<Diagnostic> {
    let mut diagnostics =
        crate::diagnostics::Diagnostics::new(session.options.limits.max_diagnostics);
    let root = Address {
        uri: session.root_uri.clone(),
        pointer: String::new(),
    };
    if session.options.validate.schema {
        match validator(session.dialect) {
            Ok(validator) => match crate::bundle::bundle_with_sources(session) {
                Ok(bundle) => {
                    let Some(root_validator) = validator.get("#") else {
                        return vec![error_at(
                            session,
                            "VALIDATOR_INITIALIZATION",
                            "Root validator is missing".into(),
                            root.clone(),
                        )];
                    };
                    for error in root_validator
                        .iter_errors(&bundle.document)
                        .take(session.options.limits.max_diagnostics)
                    {
                        let path = error.instance_path().to_string();
                        let origin = bundle
                            .origins
                            .iter()
                            .filter(|(p, _)| {
                                path == **p
                                    || path
                                        .strip_prefix(p.as_str())
                                        .is_some_and(|tail| tail.starts_with('/'))
                            })
                            .max_by_key(|(p, _)| p.len());
                        let at = origin.map_or_else(
                            || Address {
                                uri: root.uri.clone(),
                                pointer: path.clone(),
                            },
                            |(prefix, address)| Address {
                                uri: address.uri.clone(),
                                pointer: format!("{}{}", address.pointer, &path[prefix.len()..]),
                            },
                        );
                        diagnostics.push(error_at(
                            session,
                            "OPENAPI_SCHEMA",
                            error.to_string(),
                            at,
                        ));
                    }
                }
                Err(error) => diagnostics.extend(error.diagnostics),
            },
            Err(error) => diagnostics.push(error_at(
                session,
                "VALIDATOR_INITIALIZATION",
                error.clone(),
                root.clone(),
            )),
        }
        if let Ok(validators) = validator(session.dialect) {
            validate_reference_targets(session, validators, &mut diagnostics);
        }
        if session.dialect.modern_schema() {
            let mut candidates = Vec::new();
            for (address, context) in &session.contexts {
                if context.context != Context::Schema {
                    continue;
                }
                let parent_is_schema = address
                    .pointer
                    .rsplit_once('/')
                    .and_then(|(p, _)| {
                        session.contexts.get(&Address {
                            uri: address.uri.clone(),
                            pointer: p.into(),
                        })
                    })
                    .is_some_and(|c| {
                        matches!(
                            c.context,
                            Context::Schema | Context::SchemaArray | Context::SchemaMap
                        )
                    });
                // One metaschema check per Schema Object root covers descendants;
                // component maps are collections, so each entry is its own root.
                let parent_is_map = address
                    .pointer
                    .rsplit_once('/')
                    .and_then(|(p, _)| {
                        session.contexts.get(&Address {
                            uri: address.uri.clone(),
                            pointer: p.into(),
                        })
                    })
                    .is_some_and(|c| c.context == Context::SchemaMap);
                if !parent_is_schema
                    || parent_is_map
                    || session
                        .value(address)
                        .is_some_and(|v| v.get("$schema").is_some())
                {
                    candidates.push(address);
                }
            }
            // On WASM the dependency returns an owned, freshly compiled
            // meta-schema validator. Reuse that handle across this analysis;
            // constructing one per schema root makes validation scale with
            // compilation cost instead of document size.
            let mut meta_validator = None;
            let mut draft7_validator = None;
            let mut draft201909_validator = None;
            let mut draft4_validator = None;
            for address in candidates {
                if diagnostics.full() {
                    break;
                }
                let Some(value) = session.value(address) else {
                    continue;
                };
                let default_dialect = session
                    .sources
                    .get(&address.uri)
                    .and_then(|s| s.document.get("jsonSchemaDialect"))
                    .and_then(Value::as_str);
                if value.get("$schema").is_none()
                    && default_dialect.is_some_and(|uri| !known_dialect(uri))
                {
                    continue;
                }
                let mut value = value.clone();
                // OpenAPI's base dialect permits the OpenAPI vocabulary. This
                // checks standard 2020-12 keyword shapes, not custom vocabularies.
                let schema_dialect = session
                    .contexts
                    .get(address)
                    .map(|node| node.schema_dialect)
                    .unwrap_or_default();
                strip_dialect(&mut value, schema_dialect);
                let validator = match schema_dialect {
                    SchemaDialect::Draft4 => {
                        draft4_validator.get_or_insert_with(jsonschema::draft4::meta::validator)
                    }
                    SchemaDialect::Draft7 => {
                        draft7_validator.get_or_insert_with(jsonschema::draft7::meta::validator)
                    }
                    SchemaDialect::Draft201909 => draft201909_validator
                        .get_or_insert_with(jsonschema::draft201909::meta::validator),
                    SchemaDialect::Draft202012 => {
                        meta_validator.get_or_insert_with(jsonschema::draft202012::meta::validator)
                    }
                };
                if let Err(error) = validator.validate(&value) {
                    let at = Address {
                        uri: address.uri.clone(),
                        pointer: format!("{}{}", address.pointer, error.instance_path()),
                    };
                    diagnostics.push(error_at(session, "JSON_SCHEMA", error.to_string(), at));
                }
            }
        }
    }
    dialect_warnings(session, &mut diagnostics);
    if session.options.validate.spec {
        semantic(session, inspection, &mut diagnostics);
    }
    diagnostics.finish()
}

fn dialect_warnings(session: &Session<'_>, diagnostics: &mut crate::diagnostics::Diagnostics) {
    let mut check = |address: &Address, key: &str, value: &Value| {
        if diagnostics.full() {
            return;
        }
        if let Some(uri) = value.get(key).and_then(Value::as_str)
            && !known_dialect(uri)
        {
            diagnostics.push(Diagnostic {
                code: "CUSTOM_DIALECT".into(),
                severity: Severity::Warning,
                message: format!(
                    "Custom dialect {uri} is preserved; its vocabulary is not evaluated"
                ),
                location: session.locate(&Address {
                    uri: address.uri.clone(),
                    pointer: pointer::child(&address.pointer, key),
                }),
                related: Vec::new(),
            });
        }
    };
    for source in session.sources.values() {
        let address = Address {
            uri: source.uri.clone(),
            pointer: String::new(),
        };
        if let Some(value) = session.value(&address)
            && syntax::root_context(value) != Context::Schema
        {
            check(&address, "jsonSchemaDialect", value);
        }
    }
    // Scope indexing already identified every Schema Object. Avoid another full
    // syntax traversal just to find dialect declarations.
    for (address, context) in &session.contexts {
        if context.context == Context::Schema
            && let Some(value) = session.value(address)
        {
            check(address, "$schema", value);
        }
    }
}

fn validate_reference_targets(
    session: &Session<'_>,
    validators: &jsonschema::ValidatorMap,
    diagnostics: &mut crate::diagnostics::Diagnostics,
) {
    let mut checked = BTreeSet::new();
    for edge in &session.references {
        let Some(target) = edge.target.as_ref() else {
            continue;
        };
        let holder = Address {
            uri: edge.from.address.uri.clone(),
            pointer: edge
                .from
                .address
                .pointer
                .rsplit_once('/')
                .map_or("", |(p, _)| p)
                .into(),
        };
        let Some(context) = session.contexts.get(&holder) else {
            continue;
        };
        let Some(kind) = reference_kind(&holder.pointer, context.context, session.value(&holder))
        else {
            continue;
        };
        let target = session.follow(target.address.clone());
        if !checked.insert((target.clone(), kind)) {
            continue;
        }
        let Some(value) = session.value(&target) else {
            continue;
        };
        if kind != "schema" && value.get("$ref").is_some() {
            continue;
        }
        let definition = match session.dialect {
            Dialect::Swagger2 => match kind {
                "path-item" => "pathItem",
                "security-scheme" => continue,
                "request-body" | "example" | "link" | "callbacks" | "media-type" => continue,
                other => other,
            },
            Dialect::OpenApi30 => match kind {
                "path-item" => "PathItem",
                "security-scheme" => "SecurityScheme",
                "request-body" => "RequestBody",
                "schema" => "Schema",
                "parameter" => "Parameter",
                "response" => "Response",
                "header" => "Header",
                "example" => "Example",
                "link" => "Link",
                "callbacks" => "Callback",
                "media-type" => "MediaType",
                _ => continue,
            },
            _ => kind,
        };
        let definitions = if session.dialect.modern_schema() {
            "$defs"
        } else {
            "definitions"
        };
        let Some(validator) = validators.get(&format!("#/{definitions}/{definition}")) else {
            continue;
        };
        let remaining = session
            .options
            .limits
            .max_diagnostics
            .saturating_sub(diagnostics.len());
        for error in validator.iter_errors(value).take(remaining) {
            let address = Address {
                uri: target.uri.clone(),
                pointer: format!("{}{}", target.pointer, error.instance_path()),
            };
            let mut diagnostic = error_at(
                session,
                "REFERENCE_TARGET_SCHEMA",
                format!("Invalid {kind} reference target: {error}"),
                address,
            );
            diagnostic.related.push(edge.from.clone());
            diagnostics.push(diagnostic);
        }
    }
}

fn reference_kind(pointer: &str, context: Context, value: Option<&Value>) -> Option<&'static str> {
    if context == Context::Schema {
        return Some("schema");
    }
    if context != Context::Api {
        return None;
    }
    let parts: Vec<_> = pointer.split('/').collect();
    if value.is_some_and(|value| syntax::is_path_item(pointer, value)) {
        return Some("path-item");
    }
    if parts.last() == Some(&"requestBody") {
        return Some("request-body");
    }
    match parts.iter().rev().nth(1).copied() {
        Some("responses") => Some("response"),
        Some("parameters") => Some("parameter"),
        Some("headers") => Some("header"),
        Some("requestBodies") => Some("request-body"),
        Some("securitySchemes") => Some("security-scheme"),
        Some("examples") => Some("example"),
        Some("links") => Some("link"),
        Some("callbacks") => Some("callbacks"),
        Some("content") => Some("media-type"),
        _ => None,
    }
}

fn semantic(
    session: &Session<'_>,
    inspection: &Inspection,
    diagnostics: &mut crate::diagnostics::Diagnostics,
) {
    let mut identifiers: BTreeMap<&str, Location> = BTreeMap::new();
    let security_schemes = inspection.security_schemes.as_object();
    for operation in &inspection.operations {
        if diagnostics.full() {
            break;
        }
        let at = &operation.location.address;
        if let Some(id) = operation.operation_id.as_deref()
            && let Some(previous) = identifiers.insert(id, operation.location.clone())
        {
            let mut diagnostic = error_at(
                session,
                "DUPLICATE_OPERATION_ID",
                format!("Duplicate operationId {id}"),
                at.clone(),
            );
            diagnostic.related.push(previous);
            diagnostics.push(diagnostic);
        }
        let declared: BTreeSet<_> = operation
            .parameters
            .iter()
            .filter(|p| p.get("in").and_then(Value::as_str) == Some("path"))
            .filter_map(|p| p.get("name").and_then(Value::as_str))
            .collect();
        let placeholders = placeholders(&operation.path);
        if operation.path.matches('{').count() > placeholders.len() {
            diagnostics.push(error_at(
                session,
                "DUPLICATE_PATH_PLACEHOLDER",
                "Path template repeats a parameter name or has unmatched braces".into(),
                at.clone(),
            ));
        }
        for name in &placeholders {
            if operation.kind == "path" && !declared.contains(name.as_str()) {
                diagnostics.push(error_at(
                    session,
                    "MISSING_PATH_PARAMETER",
                    format!("Path template parameter {name} is not declared"),
                    at.clone(),
                ));
            }
        }
        for parameter in &operation.parameters {
            let name = parameter.get("name").and_then(Value::as_str).unwrap_or("");
            let location = parameter.get("in").and_then(Value::as_str).unwrap_or("");
            if location == "path" {
                if parameter.get("required").and_then(Value::as_bool) != Some(true) {
                    diagnostics.push(error_at(
                        session,
                        "PATH_PARAMETER_REQUIRED",
                        format!("Path parameter {name} must be required"),
                        at.clone(),
                    ));
                }
                if !placeholders.contains(name) {
                    diagnostics.push(error_at(
                        session,
                        "UNUSED_PATH_PARAMETER",
                        format!("Path parameter {name} is absent from template"),
                        at.clone(),
                    ));
                }
            }
        }
        for requirement in &operation.security {
            for (name, scopes) in requirement.as_object().into_iter().flatten() {
                let definition = security_schemes.and_then(|schemes| schemes.get(name));
                if definition.is_none() {
                    diagnostics.push(error_at(
                        session,
                        "UNDEFINED_SECURITY_SCHEME",
                        format!("Security scheme {name} is not defined"),
                        at.clone(),
                    ));
                }
                if let Some(definition) = definition {
                    let scheme_type = definition.get("type").and_then(Value::as_str).unwrap_or("");
                    if !matches!(scheme_type, "oauth2" | "openIdConnect")
                        && scopes.as_array().is_some_and(|s| !s.is_empty())
                    {
                        diagnostics.push(error_at(
                            session,
                            "INVALID_SECURITY_SCOPES",
                            format!("Security scheme {name} does not accept scopes"),
                            at.clone(),
                        ));
                    }
                }
            }
        }
        if session.dialect == Dialect::Swagger2 {
            swagger_operation(session, operation, diagnostics);
        }
    }
    if session.dialect == Dialect::Swagger2 {
        swagger_schemas(session, diagnostics);
    }
    // Check uniqueness before inheritance merging, which intentionally overrides
    // path-level parameters at operation scope.
    for (address, context) in &session.contexts {
        if context.context != Context::Api {
            continue;
        }
        let Some(parameters) = session
            .value(address)
            .and_then(|v| v.get("parameters"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        let mut seen = BTreeSet::new();
        for (index, _) in parameters.iter().enumerate() {
            let at = Address {
                uri: address.uri.clone(),
                pointer: pointer::child(
                    &pointer::child(&address.pointer, "parameters"),
                    &index.to_string(),
                ),
            };
            let parameter = session.value(&session.follow(at.clone()));
            if let Some(parameter) = parameter {
                let key = (
                    parameter.get("in").and_then(Value::as_str),
                    parameter.get("name").and_then(Value::as_str),
                );
                if key.0.is_some() && key.1.is_some() && !seen.insert(key) {
                    diagnostics.push(error_at(
                        session,
                        "DUPLICATE_PARAMETER",
                        "Parameter name and location must be unique within each list".into(),
                        at,
                    ));
                }
            }
        }
    }
}

fn swagger_operation(
    session: &Session<'_>,
    operation: &crate::ApiOperation,
    diagnostics: &mut crate::diagnostics::Diagnostics,
) {
    let at = &operation.location.address;
    let bodies = operation
        .parameters
        .iter()
        .filter(|p| p.get("in").and_then(Value::as_str) == Some("body"))
        .count();
    let forms = operation
        .parameters
        .iter()
        .filter(|p| p.get("in").and_then(Value::as_str) == Some("formData"))
        .count();
    if bodies > 1 {
        diagnostics.push(error_at(
            session,
            "MULTIPLE_BODY_PARAMETERS",
            "An operation can have only one body parameter".into(),
            at.clone(),
        ));
    }
    if bodies > 0 && forms > 0 {
        diagnostics.push(error_at(
            session,
            "BODY_FORM_CONFLICT",
            "Body and formData parameters cannot coexist".into(),
            at.clone(),
        ));
    }
    let consumes = session
        .value(at)
        .and_then(|op| op.get("consumes"))
        .or_else(|| {
            session
                .sources
                .get(&session.root_uri)
                .and_then(|s| s.document.get("consumes"))
        })
        .and_then(Value::as_array);
    for parameter in &operation.parameters {
        if parameter.get("type").and_then(Value::as_str) == Some("file") {
            if parameter.get("in").and_then(Value::as_str) != Some("formData") {
                diagnostics.push(error_at(
                    session,
                    "FILE_PARAMETER_LOCATION",
                    "File parameters must use formData".into(),
                    at.clone(),
                ));
            }
            let has_form_media = consumes.is_some_and(|media| {
                media.iter().filter_map(Value::as_str).any(|mime| {
                    mime.split_once('/').is_some_and(|(kind, subtype)| {
                        kind == "multipart"
                            && (subtype == "form-data" || subtype.ends_with("+form-data"))
                            || kind == "application"
                                && (subtype == "x-www-form-urlencoded"
                                    || subtype.ends_with("+x-www-form-urlencoded"))
                    })
                })
            });
            if !has_form_media {
                diagnostics.push(error_at(
                    session,
                    "FILE_CONSUMES_REQUIRED",
                    "File parameters require a form media type".into(),
                    at.clone(),
                ));
            }
        }
    }
    for status in operation
        .responses
        .keys()
        .filter(|status| !status.starts_with("x-"))
    {
        if status != "default"
            && !(status.len() == 3
                && status
                    .parse::<u16>()
                    .is_ok_and(|value| (100..=599).contains(&value)))
        {
            diagnostics.push(error_at(
                session,
                "INVALID_RESPONSE_STATUS",
                format!("Invalid HTTP response status {status}"),
                Address {
                    uri: at.uri.clone(),
                    pointer: pointer::child(&pointer::child(&at.pointer, "responses"), status),
                },
            ));
        }
    }
}

fn swagger_schemas(session: &Session<'_>, diagnostics: &mut crate::diagnostics::Diagnostics) {
    for (address, context) in &session.contexts {
        if !matches!(context.context, Context::Schema | Context::Api) {
            continue;
        }
        let Some(value) = session.value(address) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("array")
            && value.get("items").is_none()
        {
            diagnostics.push(error_at(
                session,
                "ARRAY_ITEMS_REQUIRED",
                "Array schemas and parameters require items".into(),
                address.clone(),
            ));
        }
        if context.context != Context::Schema
            || value.get("type").and_then(Value::as_str) != Some("object")
        {
            continue;
        }
        let Some(required) = value.get("required").and_then(Value::as_array) else {
            continue;
        };
        let mut properties = BTreeSet::new();
        collect_properties(
            session,
            address.clone(),
            &mut BTreeSet::new(),
            &mut properties,
        );
        for name in required.iter().filter_map(Value::as_str) {
            if !properties.contains(name) {
                diagnostics.push(error_at(
                    session,
                    "UNDECLARED_REQUIRED_PROPERTY",
                    format!("Required property {name} is not declared"),
                    address.clone(),
                ));
            }
        }
    }
}

fn collect_properties(
    session: &Session<'_>,
    address: Address,
    seen: &mut BTreeSet<Address>,
    properties: &mut BTreeSet<String>,
) {
    let address = session.follow(address);
    if seen.len() >= session.options.limits.max_nodes || !seen.insert(address.clone()) {
        return;
    }
    let Some(value) = session.value(&address) else {
        return;
    };
    if let Some(map) = value.get("properties").and_then(Value::as_object) {
        properties.extend(map.keys().cloned());
    }
    if let Some(all_of) = value.get("allOf").and_then(Value::as_array) {
        for index in 0..all_of.len() {
            collect_properties(
                session,
                Address {
                    uri: address.uri.clone(),
                    pointer: pointer::child(
                        &pointer::child(&address.pointer, "allOf"),
                        &index.to_string(),
                    ),
                },
                seen,
                properties,
            );
        }
    }
}

fn placeholders(path: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut remainder = path;
    while let Some(start) = remainder.find('{') {
        remainder = &remainder[start + 1..];
        let Some(end) = remainder.find('}') else {
            break;
        };
        names.insert(remainder[..end].to_string());
        remainder = &remainder[end + 1..];
    }
    names
}
fn known_dialect(uri: &str) -> bool {
    SchemaDialect::from_uri(uri).is_some()
}
fn strip_dialect(value: &mut Value, dialect: SchemaDialect) {
    if value
        .get("$schema")
        .and_then(Value::as_str)
        .is_some_and(|uri| SchemaDialect::from_uri(uri) != Some(dialect))
    {
        *value = Value::Bool(true);
        return;
    }
    let tuple_items = value.get("items").is_some_and(Value::is_array);
    if let Some(map) = value.as_object_mut() {
        if map.get("$schema").is_some_and(Value::is_string) {
            map.remove("$schema");
        }
        let keys: Vec<_> = map.keys().cloned().collect();
        for key in keys {
            let context = syntax::next_schema(
                Context::Schema,
                &key,
                &map[&key],
                tuple_items,
                Dialect::OpenApi31,
                dialect,
            );
            match context {
                Context::Schema => strip_dialect(&mut map[&key], dialect),
                Context::SchemaArray => {
                    if let Some(values) = map[&key].as_array_mut() {
                        for value in values {
                            strip_dialect(value, dialect);
                        }
                    }
                }
                Context::SchemaMap | Context::Dependencies => {
                    if let Some(values) = map[&key].as_object_mut() {
                        for value in values.values_mut() {
                            if value.is_object() || value.is_boolean() {
                                strip_dialect(value, dialect);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
fn error_at(session: &Session<'_>, code: &str, message: String, at: Address) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        severity: Severity::Error,
        message,
        location: session.locate(&at),
        related: Vec::new(),
    }
}
