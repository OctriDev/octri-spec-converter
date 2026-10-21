//! Built-in checks over the parser's effective inventory and immutable sources.

use crate::{Category, Edit, Finding, ProposedFix, Severity, catalogue, naming, source::Sources};
use octri_parser::{Address, ApiOperation, Dialect, Location, Report};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub struct Findings<'a> {
    pub sources: Sources<'a>,
    pub values: Vec<Finding>,
    pub limit: usize,
    pub truncated: bool,
    examples: crate::examples::Examples,
    last_inserted: bool,
    keys: BTreeMap<String, usize>,
}

impl Findings<'_> {
    pub fn push(
        &mut self,
        rule_id: &str,
        target: String,
        location: Location,
        message: impl Into<String>,
    ) {
        self.last_inserted = false;
        let key = format!("{rule_id}:{target}");
        let message = message.into();
        if let Some(index) = self.keys.get(&key) {
            let existing = &mut self.values[*index];
            if !existing.message.contains(&message) {
                existing.message.push(' ');
                existing.message.push_str(&message);
            }
            if existing.location != location && !existing.related.contains(&location) {
                existing.related.push(location);
            }
            return;
        }
        if self.values.len() >= self.limit {
            self.truncated = true;
            return;
        }
        let Some(rule) = catalogue::find(rule_id) else {
            return;
        };
        self.keys.insert(key.clone(), self.values.len());
        let fingerprint = hash(
            format!(
                "{rule_id}\0{}\0{}\0{target}",
                location.address.uri, location.address.pointer
            )
            .as_bytes(),
        );
        self.values.push(Finding {
            key,
            fingerprint,
            rule_id: rule_id.into(),
            title: rule.title.into(),
            why: rule.why.into(),
            severity: if rule.weight >= 2.0
                || rule_id == "spec-integrity"
                || rule_id == "schema-constraints"
            {
                Severity::High
            } else if rule_id == "project-size" || rule_id == "schema-composition" {
                Severity::Advice
            } else {
                Severity::Medium
            },
            target,
            message,
            fixable: false,
            ignorable: rule_id != "spec-integrity",
            location,
            related: Vec::new(),
            categories: rule.categories.to_vec(),
            impact: rule.why.into(),
            evidence: "static-analysis".into(),
            remediation: rule.remediation.into(),
            code: None,
            fix: None,
            exclusion_reason: None,
            baseline: false,
        });
        self.last_inserted = true;
    }

    fn at(&mut self, rule: &str, at: &Address, message: impl Into<String>) {
        let location = self.sources.location(at);
        self.push(
            rule,
            format!("{}#{}", at.uri, at.pointer),
            location,
            message,
        );
    }

    fn operation(&mut self, rule: &str, operation: &ApiOperation, message: impl Into<String>) {
        self.push(
            rule,
            format!("{} {}", operation.method, operation.path),
            operation.location.clone(),
            message,
        );
    }

    fn propose(&mut self, label: &str, edits: Vec<Edit>, kind: &str) {
        if !self.last_inserted {
            return;
        }
        if let Some(finding) = self.values.last_mut() {
            finding.fixable = true;
            finding.fix = Some(ProposedFix {
                label: label.into(),
                kind: kind.into(),
                field: edits
                    .first()
                    .and_then(|edit| edit.pointer.rsplit('/').next())
                    .map(str::to_owned),
                before: edits.first().and_then(|edit| edit.before.clone()),
                after: edits.first().map(|edit| edit.after.clone()),
                edits,
                placeholder: None,
                status: None,
                media_type: None,
            });
        }
    }

    fn authored(&mut self, label: &str, at: &Address, field: &str, kind: &str) {
        if !self.last_inserted {
            return;
        }
        let before = self.sources.value(at).cloned();
        self.propose(
            label,
            vec![Edit {
                uri: at.uri.clone(),
                pointer: at.pointer.clone(),
                before,
                after: Value::Null,
            }],
            kind,
        );
        if let Some(fix) = self
            .values
            .last_mut()
            .and_then(|finding| finding.fix.as_mut())
        {
            fix.field = Some(field.into());
            fix.after = None;
        }
    }
}

pub fn run(report: &Report, limit: usize) -> Findings<'_> {
    let mut findings = Findings {
        sources: Sources(report),
        values: Vec::new(),
        limit,
        truncated: false,
        examples: crate::examples::Examples::new(report),
        last_inserted: false,
        keys: BTreeMap::new(),
    };
    for diagnostic in &report.diagnostics {
        if findings.truncated {
            break;
        }
        if diagnostic.severity != octri_parser::Severity::Error {
            continue;
        }
        let target = format!(
            "{}:{}#{}",
            diagnostic.code, diagnostic.location.address.uri, diagnostic.location.address.pointer
        );
        findings.push(
            "spec-integrity",
            target,
            diagnostic.location.clone(),
            &diagnostic.message,
        );
        if findings.last_inserted
            && let Some(last) = findings.values.last_mut()
        {
            last.code = Some(diagnostic.code.clone());
            last.related = diagnostic.related.clone();
        }
    }
    if report.dialect == Some(Dialect::Swagger2) {
        return findings;
    }
    let Some(inspection) = &report.inspection else {
        return findings;
    };
    let root = Address {
        uri: report.registry.root_uri.clone(),
        pointer: String::new(),
    };
    let document = report.document.as_ref().unwrap_or(&Value::Null);
    let mut ids: BTreeMap<String, Location> = BTreeMap::new();
    let mut public_names: BTreeMap<String, Location> = BTreeMap::new();
    let mut mcp_names: BTreeMap<String, Location> = BTreeMap::new();
    let mut reserved: BTreeSet<_> = inspection
        .operations
        .iter()
        .filter_map(|operation| operation.operation_id.as_deref())
        .map(|id| naming::camel(id).to_ascii_lowercase())
        .collect();
    let mut shapes: BTreeMap<String, (BTreeSet<String>, Vec<Address>)> = BTreeMap::new();
    let mut naming_conventions = naming::NamingConventions::new(&inspection.operations);

    for operation in &inspection.operations {
        if findings.truncated {
            break;
        }
        let authored = findings
            .sources
            .value(&operation.location.address)
            .cloned()
            .unwrap_or(Value::Null);
        let effective = effective_operation(operation, &findings.sources);
        let operation = &effective;
        audit_operation(&mut findings, operation, &authored, &mut naming_conventions);
        if let Some(id) = operation
            .operation_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        {
            let normalized = naming::camel(id).to_ascii_lowercase();
            let public_name = naming::public_name(operation, &authored);
            let previous = ids
                .get(&normalized)
                .or_else(|| public_names.get(&public_name))
                .cloned();
            if let Some(previous) = previous {
                findings.operation("operation-id-unique", operation, format!("Authored identifier `{id}` collides after case/separator or namespace normalization (`{public_name}`). Review both operation locations."));
                if findings.last_inserted
                    && let Some(last) = findings.values.last_mut()
                {
                    last.related.push(previous);
                }
                let base = if naming::identifier(id) {
                    id.to_owned()
                } else {
                    naming::suggested(operation)
                };
                if let Some(candidate) =
                    (2..100_000)
                        .map(|suffix| format!("{base}_{suffix}"))
                        .find(|candidate| {
                            !reserved.contains(&naming::camel(candidate).to_ascii_lowercase())
                        })
                {
                    reserved.insert(naming::camel(&candidate).to_ascii_lowercase());
                    let at = findings
                        .sources
                        .field(&operation.location.address, "operationId");
                    findings.propose(
                        "Review the collision-free operationId",
                        vec![Edit {
                            uri: at.uri,
                            pointer: at.pointer,
                            before: authored.get("operationId").cloned(),
                            after: json!(candidate),
                        }],
                        "review-required",
                    );
                }
            }
            ids.entry(normalized)
                .or_insert_with(|| operation.location.clone());
            public_names
                .entry(public_name)
                .or_insert_with(|| operation.location.clone());
            let name = id.replace(
                |ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-' && ch != '.',
                "_",
            );
            let previous = mcp_names.insert(name.clone(), operation.location.clone());
            if previous.is_some() || id.len() > 128 || name != id || name.is_empty() {
                findings.operation("mcp-tool-name", operation, format!("Tool identifier `{id}` is duplicate or outside MCP's recommended [A-Za-z0-9_.-]{{1,128}} form. Actual generator naming must be verified."));
                if findings.last_inserted
                    && let Some(previous) = previous
                    && let Some(last) = findings.values.last_mut()
                {
                    last.related.push(previous);
                }
            }
        }
        for (status, response) in &operation.responses {
            if let Some(content) = response.get("content").and_then(Value::as_object) {
                for (media, value) in content {
                    if let Some(schema) = value.get("schema").filter(|s| s.get("$ref").is_none()) {
                        track_shape(
                            &mut shapes,
                            schema,
                            operation,
                            &format!("responses/{status}/{media}"),
                        );
                    }
                }
            }
        }
        if let Some(body) = &operation.request_body
            && let Some(content) = body.get("content").and_then(Value::as_object)
        {
            for (media, value) in content {
                if let Some(schema) = value.get("schema").filter(|s| s.get("$ref").is_none()) {
                    track_shape(&mut shapes, schema, operation, media);
                }
            }
        }
    }
    for (index, previous, message) in
        naming::namespace_conflicts(&inspection.operations, &findings.sources)
    {
        if findings.truncated {
            break;
        }
        let operation = &inspection.operations[index];
        findings.push(
            "operation-id-unique",
            format!(
                "{} {}#namespace:{}#{}",
                operation.method,
                operation.path,
                inspection.operations[previous].location.address.uri,
                inspection.operations[previous].location.address.pointer
            ),
            operation.location.clone(),
            message,
        );
        if findings.last_inserted
            && let Some(last) = findings.values.last_mut()
        {
            last.related
                .push(inspection.operations[previous].location.clone());
        }
    }
    for (_, (operations, addresses)) in shapes {
        if operations.len() > 1 {
            for address in addresses {
                findings.at("shared-schemas-referenced", &address, "This inline object shape appears in several operations. Confirm it represents one shared domain model before extracting it.");
            }
        }
    }
    audit_paths(&mut findings, document, &root);
    if !inspection.operations.iter().any(|operation| {
        operation
            .servers
            .iter()
            .any(|server| text(server.get("url")))
    }) && !document
        .get("servers")
        .and_then(Value::as_array)
        .is_some_and(|servers| servers.iter().any(|server| text(server.get("url"))))
    {
        findings.push("server-url", "servers".into(), findings.sources.location(&root), "No explicit server URL is declared. OpenAPI's implicit relative server depends on the consumer's origin.");
    }
    if inspection
        .security_schemes
        .as_object()
        .is_none_or(|schemes| schemes.is_empty())
    {
        findings.push("security-declared", "components.securitySchemes".into(), findings.sources.location(&root), "No authentication scheme is declared in components.securitySchemes. An empty security requirement marks an operation public but does not define SDK authentication support. This check does not certify runtime security.");
    }
    if !package_version(
        document
            .pointer("/info/version")
            .and_then(Value::as_str)
            .unwrap_or(""),
    ) {
        findings.at("package-version-source", &findings.sources.field(&findings.sources.field(&root, "info"), "version"), "API metadata is not a semantic package version. Keep source metadata intact and configure package versions explicitly.");
    }
    for address in findings.sources.schemas() {
        audit_schema(&mut findings, &address, report.dialect);
    }
    audit_media_examples(&mut findings, report.dialect);
    if inspection.statistics.operations > 300 || inspection.statistics.schemas > 400 {
        findings.push(
            "project-size",
            format!(
                "{} operations, {} models",
                inspection.statistics.operations, inspection.statistics.schemas
            ),
            findings.sources.location(&root),
            "Consider splitting generated packages by intentionally independent API groups.",
        );
    }
    findings
}

fn text(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
}

fn audit_operation(
    findings: &mut Findings<'_>,
    operation: &ApiOperation,
    authored: &Value,
    naming_conventions: &mut naming::NamingConventions,
) {
    if !matches!(
        operation.method.as_str(),
        "GET" | "PUT" | "POST" | "DELETE" | "OPTIONS" | "HEAD" | "PATCH" | "TRACE"
    ) {
        findings.operation("sdk-target-compatibility", operation, "This HTTP method requires verification with the selected SDK generator. Parsing the operation does not prove generated transport support.");
    }
    let id = operation.operation_id.as_deref().unwrap_or("");
    if id.trim().is_empty() {
        findings.operation(
            "operation-id-present",
            operation,
            "No stable operationId is declared; generated names may change when paths change.",
        );
        let proposed = naming::suggested(operation);
        findings.propose(
            "Review the proposed operationId",
            vec![Edit {
                uri: operation.location.address.uri.clone(),
                pointer: crate::source::child(&operation.location.address.pointer, "operationId"),
                before: authored.get("operationId").cloned(),
                after: json!(proposed),
            }],
            "review-required",
        );
        findings.operation("mcp-tool-name",operation,"No explicit stable tool identifier is available. The MCP generator must synthesize one from the operation path; verify the generated name before publishing.");
    } else if let Some(proposed) = naming::suggested_rename(operation, naming_conventions) {
        findings.operation("operation-id-naming", operation, format!("`{id}` repeats a namespace or action. Review `{proposed}`; renaming changes generated public APIs."));
        let at = findings
            .sources
            .field(&operation.location.address, "operationId");
        findings.propose(
            "Review the readable operationId",
            vec![Edit {
                uri: at.uri,
                pointer: at.pointer,
                before: authored.get("operationId").cloned(),
                after: json!(proposed),
            }],
            "review-required",
        );
    }
    if !text(authored.get("summary")) && !text(authored.get("description")) {
        findings.operation(
            "operation-documented",
            operation,
            "No summary or description explains the operation's behavior.",
        );
        let at = findings
            .sources
            .field(&operation.location.address, "summary");
        findings.authored("Write an operation summary", &at, "summary", "text");
    }
    if operation.tags.iter().all(|tag| tag.trim().is_empty()) {
        findings.operation(
            "operation-tagged",
            operation,
            "No tag groups this operation in generated documentation and SDKs.",
        );
        let at = findings.sources.field(&operation.location.address, "tags");
        findings.propose(
            "Review the group derived from this path",
            vec![Edit {
                uri: at.uri,
                pointer: at.pointer,
                before: authored.get("tags").cloned(),
                after: json!(naming::namespace(operation, authored)),
            }],
            "review-required",
        );
    }

    let successful: Vec<_> = operation
        .responses
        .iter()
        .filter(|(status, _)| {
            status.starts_with('2')
                && (status.len() == 3)
                && status
                    .chars()
                    .skip(1)
                    .all(|ch| ch.is_ascii_digit() || ch == 'X')
        })
        .collect();
    if successful.is_empty() {
        findings.operation(
            "response-schema",
            operation,
            "No successful response contract is declared.",
        );
    } else if successful.iter().any(|(status, response)| {
        !matches!(status.as_str(), "204" | "205")
            && operation.method != "HEAD"
            && !has_schema(response)
    }) {
        findings.operation("response-schema", operation, "At least one body-bearing success response has no schema. 204, 205 and HEAD responses are exempt.");
        let missing: Vec<_> = successful
            .iter()
            .filter(|(status, response)| {
                !matches!(status.as_str(), "204" | "205") && !has_schema(response)
            })
            .collect();
        if missing.len() == 1
            && missing[0]
                .1
                .get("content")
                .and_then(Value::as_object)
                .is_none_or(|content| {
                    content
                        .values()
                        .filter(|media| media_schema(media).is_none())
                        .count()
                        <= 1
                })
        {
            let (status, response) = missing[0];
            let at = findings.sources.follow(
                &findings.sources.field(
                    &findings
                        .sources
                        .field(&operation.location.address, "responses"),
                    status,
                ),
            );
            let media = response
                .get("content")
                .and_then(Value::as_object)
                .and_then(|content| {
                    content
                        .iter()
                        .find(|(_, media)| media_schema(media).is_none())
                        .map(|(name, _)| name.clone())
                })
                .unwrap_or_else(|| "application/json".into());
            findings.authored(
                "Define the success response schema",
                &at,
                &format!("responses.{status}.content.{media}.schema"),
                "response-schema",
            );
            if findings.last_inserted
                && let Some(fix) = findings
                    .values
                    .last_mut()
                    .and_then(|finding| finding.fix.as_mut())
            {
                fix.status = Some((*status).clone());
                fix.media_type = Some(media);
            }
        }
    }
    if !operation
        .responses
        .keys()
        .any(|status| status == "default" || status.starts_with('4') || status.starts_with('5'))
    {
        findings.operation(
            "error-responses",
            operation,
            "No 4xx, 5xx or default response describes failure behavior.",
        );
    }
    for parameter in &operation.parameters {
        let name = parameter.get("name").and_then(Value::as_str).unwrap_or("");
        let place = parameter.get("in").and_then(Value::as_str).unwrap_or("");
        if place == "querystring" {
            findings.operation("sdk-target-compatibility", operation, "A querystring parameter describes the entire query string. Verify that the selected generator preserves its media-type serialization.");
            findings.operation("mcp-input-schema", operation, "A querystring parameter needs an explicit mapping between MCP arguments and the whole HTTP query string.");
        }
        if !text(parameter.get("description")) {
            let location = findings.sources.parameter(&operation.location, name, place);
            findings.push(
                "parameters-documented",
                format!("{} {}#{place}:{name}", operation.method, operation.path),
                location.clone(),
                format!("Parameter `{name}` has no description or IDE guidance."),
            );
            let at = findings.sources.field(&location.address, "description");
            findings.authored(
                "Write the parameter description",
                &at,
                &format!("parameters.{name}.description"),
                "text",
            );
        }
        if place == "path" && parameter.get("required") != Some(&Value::Bool(true)) {
            let location = findings.sources.parameter(&operation.location, name, place);
            let at = findings.sources.field(&location.address, "required");
            findings.at(
                "path-parameter-required",
                &at,
                format!("Path parameter `{name}` must explicitly set required: true."),
            );
            findings.propose(
                "Set required: true",
                vec![Edit {
                    uri: at.uri,
                    pointer: at.pointer,
                    before: parameter.get("required").cloned(),
                    after: Value::Bool(true),
                }],
                "deterministic",
            );
        }
    }
    for name in placeholders(&operation.path) {
        if !operation
            .parameters
            .iter()
            .any(|parameter| parameter["in"] == "path" && parameter["name"] == name)
        {
            findings.push("path-parameters-declared",format!("{} {}#{name}",operation.method,operation.path),operation.location.clone(),format!("Path placeholder `{{{name}}}` has no effective path parameter declaration. Its actual type must be authored."));
        }
    }
    if let Some(body) = &operation.request_body {
        if body
            .get("content")
            .and_then(Value::as_object)
            .is_some_and(|content| {
                content
                    .values()
                    .any(|media| media.get("itemSchema").is_some())
            })
        {
            findings.operation("sdk-target-compatibility", operation, "A sequential request body uses itemSchema. Verify streaming input support with the selected generator.");
            findings.operation(
                "mcp-input-schema",
                operation,
                "A sequential request body requires an explicit MCP argument and stream mapping.",
            );
        }
        if !has_schema(body) {
            findings.operation("request-schema",operation,"A request body is declared without a schema. Docs, typed clients and MCP arguments cannot describe its payload reliably.");
        }
        let properties_documented =
            body.get("content")
                .and_then(Value::as_object)
                .is_some_and(|content| {
                    content.values().any(|media| {
                        text(media.pointer("/schema/description"))
                            || media
                                .pointer("/schema/properties")
                                .and_then(Value::as_object)
                                .is_some_and(|props| {
                                    !props.is_empty()
                                        && props
                                            .values()
                                            .all(|property| text(property.get("description")))
                                })
                    })
                });
        if !text(body.get("description")) && !properties_documented {
            findings.operation(
                "request-body-documented",
                operation,
                "The request body and its properties have no useful documentation.",
            );
            let body = findings.sources.follow(
                &findings
                    .sources
                    .field(&operation.location.address, "requestBody"),
            );
            let at = findings.sources.field(&body, "description");
            findings.authored(
                "Add a request-body description",
                &at,
                "requestBody.description",
                "text",
            );
        }
        if body
            .get("content")
            .and_then(Value::as_object)
            .is_some_and(|content| {
                content.values().any(|media| {
                    let schema = &media["schema"];
                    schema
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind != "object")
                        || schema.get("oneOf").is_some()
                        || schema.get("anyOf").is_some()
                })
            })
        {
            findings.operation("mcp-input-schema", operation, "Non-object or alternative request bodies require an explicit MCP argument mapping; static analysis cannot prove that mapping.");
        }
    }
    let mut names = BTreeSet::new();
    if operation
        .parameters
        .iter()
        .filter_map(|parameter| parameter.get("name").and_then(Value::as_str))
        .any(|name| !names.insert(name))
    {
        findings.operation("mcp-input-schema", operation, "Parameters in different locations share a name; a flat MCP argument object needs explicit disambiguation.");
    }
    let examples = operation.responses.values().any(has_example)
        || operation.request_body.as_ref().is_some_and(has_example);
    if !examples && (!successful.is_empty() || operation.request_body.is_some()) {
        findings.operation(
            "operation-examples",
            operation,
            "No request or response media example is declared.",
        );
    }
}

/// Resolve API Media Type references without expanding any Schema Object.
fn effective_operation(operation: &ApiOperation, sources: &Sources<'_>) -> ApiOperation {
    let mut operation = operation.clone();
    let base = operation.location.address.clone();
    for (status, response) in &mut operation.responses {
        let at = sources.follow(&sources.field(&sources.field(&base, "responses"), status));
        resolve_media_types(response, &at, sources);
    }
    if let Some(body) = &mut operation.request_body {
        let at = sources.follow(&sources.field(&base, "requestBody"));
        resolve_media_types(body, &at, sources);
    }
    operation
}

fn resolve_media_types(body: &mut Value, at: &Address, sources: &Sources<'_>) {
    if let Some(content) = body.get_mut("content").and_then(Value::as_object_mut) {
        for (name, media) in content {
            let target = sources.follow(&sources.field(&sources.field(at, "content"), name));
            if let Some(value) = sources.value(&target) {
                *media = value.clone();
            }
        }
    }
}

fn media_schema(media: &Value) -> Option<&Value> {
    media.get("schema").or_else(|| media.get("itemSchema"))
}

fn has_schema(body: &Value) -> bool {
    body.get("content")
        .and_then(Value::as_object)
        .is_some_and(|content| {
            !content.is_empty() && content.values().all(|media| media_schema(media).is_some())
        })
        || body.get("schema").is_some()
}
fn has_example(body: &Value) -> bool {
    body.get("content")
        .and_then(Value::as_object)
        .is_some_and(|content| {
            content.values().any(|media| {
                media.get("example").is_some()
                    || media
                        .get("examples")
                        .and_then(Value::as_object)
                        .is_some_and(|map| !map.is_empty())
                    || media.pointer("/schema/example").is_some()
                    || media.pointer("/itemSchema/example").is_some()
                    || media
                        .pointer("/schema/examples")
                        .and_then(Value::as_array)
                        .is_some_and(|values| !values.is_empty())
                    || media
                        .pointer("/itemSchema/examples")
                        .and_then(Value::as_array)
                        .is_some_and(|values| !values.is_empty())
            })
        })
}

fn placeholders(path: &str) -> Vec<&str> {
    let mut rest = path;
    let mut names = Vec::new();
    while let Some((_, tail)) = rest.split_once('{') {
        let Some((name, tail)) = tail.split_once('}') else {
            break;
        };
        names.push(name);
        rest = tail;
    }
    names
}

fn audit_paths(findings: &mut Findings<'_>, document: &Value, root: &Address) {
    let Some(paths) = document.get("paths").and_then(Value::as_object) else {
        return;
    };
    let mut equivalent = BTreeMap::new();
    for path in paths.keys().filter(|path| path.starts_with('/')) {
        let mut normalized = path.to_owned();
        for name in placeholders(path) {
            normalized = normalized.replace(&format!("{{{name}}}"), "{}");
        }
        // Paths are case-sensitive and trailing slash behavior belongs to the
        // server. Only equivalent OpenAPI templates are collapsed here.
        if let Some(previous) = equivalent.insert(normalized, path) {
            let at = findings
                .sources
                .field(&findings.sources.field(root, "paths"), path);
            findings.at("duplicate-paths", &at, format!("`{path}` and `{previous}` are equivalent path templates regardless of parameter names."));
            let related = findings.sources.location(
                &findings
                    .sources
                    .field(&findings.sources.field(root, "paths"), previous),
            );
            if findings.last_inserted
                && let Some(last) = findings.values.last_mut()
            {
                last.related.push(related);
            }
        }
    }
}

fn track_shape(
    shapes: &mut BTreeMap<String, (BTreeSet<String>, Vec<Address>)>,
    schema: &Value,
    operation: &ApiOperation,
    _slot: &str,
) {
    if schema
        .get("properties")
        .and_then(Value::as_object)
        .is_none_or(|properties| properties.len() < 3)
    {
        return;
    }
    let key = schema.to_string();
    let entry = shapes.entry(key).or_default();
    let target = format!("{} {}", operation.method, operation.path);
    if entry.0.insert(target) {
        entry.1.push(operation.location.address.clone());
    }
}

fn package_version(value: &str) -> bool {
    semver::Version::parse(value.trim().strip_prefix('v').unwrap_or(value.trim())).is_ok()
}

fn audit_schema(findings: &mut Findings<'_>, at: &Address, dialect: Option<Dialect>) {
    let Some(schema) = findings.sources.value(at).cloned() else {
        return;
    };
    if !schema.is_object() {
        return;
    }
    let types: Vec<&str> = schema.get("type").map_or(Vec::new(), |kind| {
        kind.as_str().map_or_else(
            || {
                kind.as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect()
            },
            |kind| vec![kind],
        )
    });
    let families = [
        ("string", &["minLength", "maxLength", "pattern"][..]),
        ("array", &["minItems", "maxItems", "uniqueItems"][..]),
        ("object", &["minProperties", "maxProperties"][..]),
        (
            "number",
            &[
                "minimum",
                "maximum",
                "exclusiveMinimum",
                "exclusiveMaximum",
                "multipleOf",
            ][..],
        ),
    ];
    for (family, keywords) in families {
        if types.is_empty()
            || types.contains(&family)
            || family == "number" && types.contains(&"integer")
        {
            continue;
        }
        for keyword in keywords {
            if schema.get(keyword).is_some() {
                findings.at(
                    "schema-constraint-applicability",
                    &findings.sources.field(at, keyword),
                    format!(
                        "`{keyword}` does not constrain the declared type(s) {}.",
                        types.join(", ")
                    ),
                );
            }
        }
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        for (index, value) in values.iter().enumerate() {
            if !allows_type(&schema, value, dialect) {
                findings.at("schema-enum-domain", &findings.sources.field(&findings.sources.field(at, "enum"), &index.to_string()), "Enum member cannot inhabit the declared wire type; values are never silently coerced.");
            }
        }
    }
    for (min, max) in [
        ("minLength", "maxLength"),
        ("minItems", "maxItems"),
        ("minProperties", "maxProperties"),
        ("minimum", "maximum"),
    ] {
        if let (Some(minimum), Some(maximum)) = (
            schema.get(min).and_then(Value::as_number),
            schema.get(max).and_then(Value::as_number),
        ) && (octri_parser::numbers::compare(minimum, maximum).is_gt()
            || min == "minimum"
                && octri_parser::numbers::compare(minimum, maximum).is_eq()
                && (schema["exclusiveMinimum"] == true || schema["exclusiveMaximum"] == true))
        {
            findings.at(
                "schema-constraints",
                at,
                format!("`{min}` and `{max}` describe an empty value domain."),
            );
        }
    }
    if let (Some(min), Some(max)) = (
        schema.get("exclusiveMinimum").and_then(Value::as_number),
        schema.get("exclusiveMaximum").and_then(Value::as_number),
    ) && !octri_parser::numbers::compare(min, max).is_lt()
    {
        findings.at(
            "schema-constraints",
            at,
            "Exclusive numeric bounds describe an empty value domain.",
        );
    }
    if let Some(constant) = schema.get("const")
        && let Some(values) = schema.get("enum").and_then(Value::as_array)
        && !values.contains(constant)
    {
        findings.at(
            "schema-constraints",
            at,
            "The const value is excluded by enum.",
        );
    }
    if matches!(dialect, Some(Dialect::OpenApi31 | Dialect::OpenApi32))
        && schema.get("nullable").is_some()
    {
        findings.at("schema-dialect", &findings.sources.field(at, "nullable"), "OpenAPI 3.1 and 3.2 use JSON Schema null types. The OpenAPI 3.0 nullable keyword does not declare nullability here.");
    }
    if schema.get("oneOf").is_some()
        || schema.get("anyOf").is_some()
        || schema.get("allOf").is_some()
        || schema.get("discriminator").is_some()
        || schema.get("$dynamicRef").is_some()
    {
        findings.at("schema-composition", at, "Composition, discriminator or dynamic-reference behavior requires target-specific generator verification. Static findings do not flatten alternatives or prove runtime behavior.");
    }
    for profile_sensitive in [
        "unevaluatedProperties",
        "patternProperties",
        "dependentSchemas",
        "contentEncoding",
        "$dynamicRef",
    ] {
        if schema.get(profile_sensitive).is_some() {
            findings.at("sdk-target-compatibility", &findings.sources.field(at, profile_sensitive), format!("`{profile_sensitive}` has no universal native SDK representation. Consult the selected versioned profile and execute the generator."));
        }
    }
    if let Some(example) = schema.get("example") {
        validate_example(
            findings,
            &schema,
            example,
            &findings.sources.field(at, "example"),
            at,
            dialect,
        );
    }
    if let Some(examples) = schema.get("examples").and_then(Value::as_array) {
        for (index, example) in examples.iter().enumerate() {
            validate_example(
                findings,
                &schema,
                example,
                &findings
                    .sources
                    .field(&findings.sources.field(at, "examples"), &index.to_string()),
                at,
                dialect,
            );
        }
    }
}

fn allows_type(schema: &Value, value: &Value, dialect: Option<Dialect>) -> bool {
    if value.is_null()
        && matches!(dialect, Some(Dialect::OpenApi30 | Dialect::Swagger2))
        && schema["nullable"] == true
    {
        return true;
    }
    let kind = schema.get("type");
    let allows = |kind: &str| match kind {
        "null" => value.is_null(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "number" => value.is_number(),
        "integer" => value
            .as_number()
            .is_some_and(octri_parser::numbers::is_integer),
        "object" => value.is_object(),
        "array" => value.is_array(),
        _ => true,
    };
    kind.is_none_or(|kind| {
        kind.as_str().map_or_else(
            || {
                kind.as_array()
                    .is_none_or(|kinds| kinds.iter().filter_map(Value::as_str).any(allows))
            },
            allows,
        )
    })
}

fn validate_example(
    findings: &mut Findings<'_>,
    schema: &Value,
    example: &Value,
    at: &Address,
    schema_at: &Address,
    dialect: Option<Dialect>,
) {
    if !allows_type(schema, example, dialect) {
        findings.at(
            "schema-example-type",
            at,
            "Authored example has a different type from its schema.",
        );
        return;
    }
    match findings.examples.validate(example, schema_at) {
        crate::examples::ExampleResult::Valid => {}
        crate::examples::ExampleResult::Invalid(error) => findings.at(
            "schema-example-valid",
            at,
            format!("Example violates its schema: {error}"),
        ),
        crate::examples::ExampleResult::Unsupported(error) => findings.at(
            "schema-example-unverified",
            at,
            format!("Example could not be verified: {error}"),
        ),
    }
}

fn audit_media_examples(findings: &mut Findings<'_>, dialect: Option<Dialect>) {
    let roots = findings.sources.schemas();
    for schema_at in roots
        .into_iter()
        .filter(|at| at.pointer.ends_with("/schema") || at.pointer.ends_with("/itemSchema"))
    {
        let Some((media_pointer, _)) = schema_at.pointer.rsplit_once('/') else {
            continue;
        };
        let media_at = Address {
            uri: schema_at.uri.clone(),
            pointer: media_pointer.into(),
        };
        let Some(media) = findings.sources.value(&media_at).cloned() else {
            continue;
        };
        let item_schema = schema_at.pointer.ends_with("/itemSchema");
        let Some(schema) = media.get(if item_schema { "itemSchema" } else { "schema" }) else {
            continue;
        };
        if let Some(example) = media.get("example") {
            validate_media_example(
                findings,
                schema,
                example,
                &findings.sources.field(&media_at, "example"),
                &schema_at,
                dialect,
                item_schema,
            );
        }
        if let Some(examples) = media.get("examples").and_then(Value::as_object) {
            for name in examples.keys() {
                let at = findings.sources.follow(
                    &findings
                        .sources
                        .field(&findings.sources.field(&media_at, "examples"), name),
                );
                let authored = findings.sources.value(&at).cloned().unwrap_or(Value::Null);
                let field = if authored.get("dataValue").is_some() {
                    "dataValue"
                } else {
                    "value"
                };
                if let Some(example) = authored.get(field) {
                    validate_media_example(
                        findings,
                        schema,
                        example,
                        &findings.sources.field(&at, field),
                        &schema_at,
                        dialect,
                        item_schema,
                    );
                } else if authored.get("externalValue").is_some() {
                    findings.at("schema-example-unverified", &findings.sources.field(&at, "externalValue"), "External example values are not retrieved. Supply an inline dataValue for offline validation.");
                } else if let Some(serialized) = authored.get("serializedValue") {
                    let at = findings.sources.field(&at, "serializedValue");
                    let media_type = media_at
                        .pointer
                        .rsplit_once("/content/")
                        .map(|(_, name)| name.replace("~1", "/").replace("~0", "~"));
                    if media_type
                        .as_deref()
                        .is_some_and(|name| name == "application/json" || name.ends_with("+json"))
                        && !item_schema
                    {
                        match serialized
                            .as_str()
                            .and_then(|value| serde_json::from_str::<Value>(value).ok())
                        {
                            Some(value) => {
                                validate_example(findings, schema, &value, &at, &schema_at, dialect)
                            }
                            None => findings.at(
                                "schema-example-valid",
                                &at,
                                "Serialized JSON example is not valid JSON.",
                            ),
                        }
                    } else {
                        findings.at("schema-example-unverified", &at, "Wire serialization cannot be inferred safely here. Supply dataValue for offline schema validation; verify the serialized representation with the consumer.");
                    }
                }
            }
        }
    }
}

fn validate_media_example(
    findings: &mut Findings<'_>,
    schema: &Value,
    value: &Value,
    at: &Address,
    schema_at: &Address,
    dialect: Option<Dialect>,
    items: bool,
) {
    if items {
        if let Some(values) = value.as_array() {
            for (index, value) in values.iter().enumerate() {
                validate_example(
                    findings,
                    schema,
                    value,
                    &findings.sources.field(at, &index.to_string()),
                    schema_at,
                    dialect,
                );
            }
        } else {
            findings.at("schema-example-unverified", at, "A sequential media example needs a dataValue array of logical items for offline itemSchema validation.");
        }
    } else {
        validate_example(findings, schema, value, at, schema_at, dialect);
    }
}

pub fn categories_selected(categories: &[Category], profiles: &[String]) -> bool {
    categories.contains(&Category::Integrity)
        || categories.iter().any(|category| {
            profiles.iter().any(|profile| match category {
                Category::Integrity => true,
                Category::Docs => profile == "docs",
                Category::Sdk => profile == "sdk" || profile.starts_with("sdk-"),
                Category::Mcp => profile == "mcp",
            })
        })
}
