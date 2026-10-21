#![allow(clippy::unwrap_used)]
use octri_parser::{Input, NoIo, Operation, Options, Request, ValidationMode, execute};
use serde_json::json;

#[test]
fn openapi32_media_schemas_are_references_but_example_data_is_literal() {
    let document = json!({
        "openapi":"3.2.1","info":{"title":"Stream","version":"1"},
        "paths":{"/events":{"additionalOperations":{"COPY":{"responses":{"200":{"description":"Events","content":{"application/jsonl":{"$ref":"#/components/mediaTypes/Events"}}}}}}}},
        "components":{"schemas":{"Event":{"type":"object"}},"mediaTypes":{"Events":{
            "itemSchema":{"$ref":"#/components/schemas/Event"},
            "examples":{"events":{"dataValue":[{"$ref":"https://example.invalid/data","$id":"literal"}]}}
        }}}
    });
    let mut options = Options::default();
    options.validate.mode = ValidationMode::Strict;
    let report = execute(
        Request {
            operation: Operation::Analyze,
            input: Input {
                uri: "memory://tests/openapi.json".into(),
                content: Some(document.to_string()),
                generated: false,
            },
            options,
        },
        &mut NoIo,
    )
    .unwrap();
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.inspection.unwrap().operations[0].method, "COPY");
    assert!(
        report
            .references
            .iter()
            .any(|edge| edge.kind == "$ref"
                && edge.from.address.pointer.ends_with("/itemSchema/$ref"))
    );
    assert!(
        report
            .references
            .iter()
            .all(|edge| !edge.from.address.pointer.contains("/dataValue/"))
    );
}

#[test]
fn custom_methods_keep_case_and_metadata_at_their_physical_source() {
    let document = json!({
        "openapi":"3.2.1", "info":{"title":"Methods","version":"1"},
        "paths":{"/events":{"$ref":"events.json"}}
    });
    let operation = |id: &str| {
        json!({
            "operationId":id, "description":format!("Description for {id}"),
            "x-interfacer-namespace":"events", "x-evidence":{"$ref":"literal-data"},
            "responses":{"200":{"description":"OK"}}
        })
    };
    let path = json!({
        "get":operation("listEvents"), "query":operation("queryEvents"),
        "additionalOperations":{"COPY":operation("copyUpper"),"copy":operation("copyLower"),"get":operation("getLower")}
    });
    let mut options = Options::default();
    options.validate.mode = ValidationMode::Strict;
    options
        .files
        .insert("memory://tests/events.json".into(), path.to_string());
    let report = execute(
        Request {
            operation: Operation::Analyze,
            input: Input {
                uri: "memory://tests/openapi.json".into(),
                content: Some(document.to_string()),
                generated: false,
            },
            options,
        },
        &mut NoIo,
    )
    .unwrap();
    assert!(report.valid, "{:?}", report.diagnostics);
    let inventory = report.inspection.unwrap();
    let methods: Vec<_> = inventory
        .operations
        .iter()
        .map(|operation| operation.method.as_str())
        .collect();
    assert_eq!(methods, ["GET", "QUERY", "COPY", "copy", "get"]);
    for operation in inventory.operations {
        let id = operation.operation_id.unwrap();
        assert_eq!(
            operation.description.as_deref(),
            Some(format!("Description for {id}").as_str())
        );
        assert_eq!(
            operation.extensions.get("x-interfacer-namespace"),
            Some(&json!("events"))
        );
        assert_eq!(
            operation.extensions.get("x-evidence"),
            Some(&json!({"$ref":"literal-data"}))
        );
        assert_eq!(operation.location.address.uri, "memory://tests/events.json");
    }
}
