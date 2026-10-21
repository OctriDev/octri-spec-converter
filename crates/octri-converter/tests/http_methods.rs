use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::json;
use std::collections::BTreeSet;

fn request(content: String, name: &str, target: &str) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: format!("memory://methods/{name}"),
            content: Some(content),
            generated: false,
        },
        files: Default::default(),
        config: Config {
            target_version: Some(target.into()),
            ..Config::default()
        },
    }
}

fn observations(methods: &[&str]) -> Vec<(&'static str, String)> {
    let url = "https://api.example.com/pets";
    let postman_items: Vec<_> = methods
        .iter()
        .map(|method| {
            json!({
                "name": method,
                "request": { "method": method, "url": url }
            })
        })
        .collect();
    let har_entries: Vec<_> = methods
        .iter()
        .map(|method| {
            json!({
                "request": { "method": method, "url": url, "headers": [] },
                "response": {
                    "status": 204,
                    "statusText": "Empty",
                    "headers": [],
                    "content": { "mimeType": "text/plain", "text": "" }
                }
            })
        })
        .collect();
    let insomnia_requests: Vec<_> = methods
        .iter()
        .map(|method| {
            json!({
                "name": method,
                "method": method,
                "url": url,
                "authentication": { "type": "none" }
            })
        })
        .collect();
    let mut insomnia_resources = vec![json!({
        "_id": "workspace", "_type": "workspace", "name": "Methods"
    })];
    for (index, request) in insomnia_requests.iter().enumerate() {
        let mut resource = request.clone();
        resource["_id"] = json!(format!("request{index}"));
        resource["_type"] = json!("request");
        resource["parentId"] = json!("workspace");
        insomnia_resources.push(resource);
    }
    vec![
        (
            "requests.curl",
            methods
                .iter()
                .map(|method| format!("curl -X '{method}' {url}\n"))
                .collect(),
        ),
        (
            "postman.json",
            json!({
                "info": {
                    "name": "Methods",
                    "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
                },
                "auth": { "type": "noauth" },
                "item": postman_items
            })
            .to_string(),
        ),
        (
            "archive.har",
            json!({ "log": { "version": "1.2", "entries": har_entries } }).to_string(),
        ),
        (
            "insomnia4.json",
            json!({
                "_type": "export", "__export_format": 4, "resources": insomnia_resources
            })
            .to_string(),
        ),
        (
            "insomnia5.json",
            json!({
                "type": "collection.insomnia.rest/5.0",
                "name": "Methods",
                "collection": insomnia_requests
            })
            .to_string(),
        ),
    ]
}

#[test]
fn observed_methods_keep_exact_wire_spelling_and_separate_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let methods = ["GET", "TRACE", "QUERY", "COPY", "copy", "get", "~COPY"];
    for (name, content) in observations(&methods) {
        let report = convert(&request(content, name, "3.2.1"))?;
        assert!(report.valid, "{name}: {:?}", report.diagnostics);
        assert_ne!(
            report.status,
            Status::Blocked,
            "{name}: {:?}",
            report.decisions
        );
        assert!(report.duplicate_requests.is_empty(), "{name}");
        let document = report.document.ok_or("missing document")?;
        for field in ["get", "trace", "query"] {
            assert!(
                document["paths"]["/pets"][field].is_object(),
                "{name}: {field}"
            );
        }
        for method in ["COPY", "copy", "get", "~COPY"] {
            assert!(
                document["paths"]["/pets"]["additionalOperations"][method].is_object(),
                "{name}: {method}"
            );
        }
        let inspection = report.inspection.ok_or("missing inventory")?;
        assert_eq!(
            inspection
                .operations
                .iter()
                .map(|operation| operation.method.as_str())
                .collect::<BTreeSet<_>>(),
            methods.into_iter().collect(),
            "{name}"
        );
        assert_eq!(
            inspection
                .operations
                .iter()
                .filter_map(|operation| operation.operation_id.as_deref())
                .collect::<BTreeSet<_>>()
                .len(),
            methods.len(),
            "{name}: operation IDs must remain unique"
        );
        assert!(
            report.decisions.iter().any(|decision| decision
                .output_pointers
                .iter()
                .any(|pointer| pointer == "/paths/~1pets/additionalOperations/~0COPY")),
            "{name}: escaped method provenance"
        );
    }
    Ok(())
}

#[test]
fn older_targets_block_custom_methods_instead_of_rewriting_them()
-> Result<(), Box<dyn std::error::Error>> {
    for target in ["2.0", "3.0.4", "3.1.2"] {
        for (name, content) in observations(&["copy"]) {
            let report = convert(&request(content, name, target))?;
            assert_eq!(report.status, Status::Blocked, "{target}: {name}");
            assert!(!report.handoff_allowed);
            assert!(
                report
                    .decisions
                    .iter()
                    .any(|decision| decision.code == format!("TARGET_{target}_UNREPRESENTABLE")),
                "{target}: {name}: {:?}",
                report.decisions
            );
        }
    }
    Ok(())
}

#[test]
fn observed_invalid_method_tokens_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    for method in ["", "GET POST", "COP/Y", "méthod", "GET\r\nInjected: true"] {
        for (name, content) in observations(&[method]) {
            let error = convert(&request(content, name, "3.2.1"))
                .err()
                .ok_or("invalid method accepted")?;
            assert_eq!(error.code, "HTTP_METHOD", "{name}: {method:?}");
        }
    }
    Ok(())
}

#[test]
fn reserved_object_keys_remain_blocked_by_parser_safety_policy()
-> Result<(), Box<dyn std::error::Error>> {
    for (name, content) in observations(&["__proto__"]) {
        let report = convert(&request(content, name, "3.2.1"))?;
        assert!(!report.valid, "{name}");
        assert!(!report.handoff_allowed, "{name}");
        assert_eq!(report.status, Status::Blocked, "{name}");
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "UNSAFE_KEY"),
            "{name}: {:?}",
            report.diagnostics
        );
    }
    Ok(())
}

#[test]
fn protocol_mapping_collisions_use_exact_method_identity() -> Result<(), Box<dyn std::error::Error>>
{
    let mut input = request(
        "type Query { upper: String lower: String customGet: String fixedGet: String }".into(),
        "schema.graphql",
        "3.2.0",
    );
    for (field, method) in [
        ("upper", "COPY"),
        ("lower", "copy"),
        ("customGet", "get"),
        ("fixedGet", "GET"),
    ] {
        input.config.http_mappings.insert(
            format!("Query.{field}"),
            serde_json::from_value(json!({
                "method": method,
                "path": "/pets",
                "server": "https://api.example.com",
                "responseStatus": "200",
                "security": []
            }))?,
        );
    }
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let inspection = report.inspection.ok_or("missing inventory")?;
    assert_eq!(
        inspection
            .operations
            .iter()
            .map(|operation| operation.method.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["COPY", "copy", "get", "GET"])
    );
    input
        .config
        .http_mappings
        .get_mut("Query.upper")
        .ok_or("missing mapping")?
        .method = "copy".into();
    assert_eq!(
        convert(&input)
            .err()
            .ok_or("duplicate mapping accepted")?
            .code,
        "HTTP_MAPPING_COLLISION"
    );
    for method in ["", "GET POST", "COP/Y", "méthod", "get\n"] {
        input
            .config
            .http_mappings
            .get_mut("Query.upper")
            .ok_or("missing mapping")?
            .method = method.into();
        assert_eq!(
            convert(&input)
                .err()
                .ok_or("invalid mapping method accepted")?
                .code,
            "HTTP_MAPPING"
        );
    }
    Ok(())
}

#[test]
fn duplicate_selection_preserves_other_method_cases() -> Result<(), Box<dyn std::error::Error>> {
    let (_, source) = observations(&["COPY", "copy", "copy"])
        .into_iter()
        .find(|(name, _)| *name == "postman.json")
        .ok_or("missing Postman fixture")?;
    let mut input = request(source, "postman.json", "3.2.1");
    let blocked = convert(&input)?;
    assert_eq!(blocked.status, Status::Blocked);
    assert_eq!(blocked.duplicate_requests.len(), 1);
    assert_eq!(blocked.duplicate_requests[0].identity, "copy /pets");
    input
        .config
        .duplicate_choices
        .insert("copy /pets".into(), "/item/2".into());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let inspection = report.inspection.ok_or("missing inventory")?;
    assert_eq!(
        inspection
            .operations
            .iter()
            .map(|operation| operation.method.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["COPY", "copy"])
    );
    Ok(())
}

#[test]
fn protocol_mappings_keep_custom_case_and_body_semantics() -> Result<(), Box<dyn std::error::Error>>
{
    for (source, name, key) in [
        (
            "type Query { pet(id: ID!): String }",
            "schema.graphql",
            "Query.pet",
        ),
        (
            "syntax = \"proto3\"; message Input { string id = 1; } message Output { string id = 1; } service Pets { rpc Read(Input) returns (Output); }",
            "service.proto",
            "Pets.Read",
        ),
    ] {
        for method in ["COPY", "copy", "get", "~COPY", "GET"] {
            let mut input = request(source.into(), name, "3.2.1");
            input.config.http_mappings.insert(key.into(), serde_json::from_value(json!({"method":method,"path":"/pets","server":"https://api.example.com","responseStatus":"200","security":[]}))?);
            let report = convert(&input)?;
            assert!(report.valid, "{name}: {method}: {:?}", report.diagnostics);
            if method == "GET" {
                assert_eq!(report.status, Status::Blocked);
                assert!(
                    report
                        .decisions
                        .iter()
                        .any(|decision| decision.code == "HTTP_INPUT_BINDINGS_REQUIRED")
                );
            } else {
                assert_ne!(
                    report.status,
                    Status::Blocked,
                    "{name}: {method}: {:?}",
                    report.decisions
                );
                let document = report.document.ok_or("missing document")?;
                assert!(
                    document["paths"]["/pets"]["additionalOperations"][method]["requestBody"]
                        .is_object(),
                    "{name}: {method}"
                );
                let inspection = report.inspection.ok_or("missing inventory")?;
                assert_eq!(inspection.operations[0].method, method);
            }
        }
    }
    Ok(())
}
