#![allow(clippy::unwrap_used, clippy::panic)]
use octri_parser::{Input, NoIo, Operation, Options, Request, execute};
use serde_json::Value;

#[test]
fn portable_fixtures_match_rust_contract() {
    let fixtures: Vec<Value> =
        serde_json::from_str(include_str!("../../../fixtures/cases.json")).unwrap();
    for fixture in fixtures {
        let content = fixture["input"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| fixture["input"].to_string());
        let mut options: Options = serde_json::from_value(
            fixture
                .get("options")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        )
        .unwrap();
        options.validate.mode = octri_parser::ValidationMode::Strict;
        let report = execute(
            Request {
                operation: Operation::Analyze,
                input: Input {
                    uri: "memory://octri/openapi.yaml".into(),
                    content: Some(content),
                    generated: false,
                },
                options,
            },
            &mut NoIo,
        )
        .unwrap();
        assert_eq!(
            report.valid,
            fixture["valid"].as_bool().unwrap(),
            "{}: {:?}",
            fixture["name"],
            report.diagnostics
        );
    }
}

#[test]
fn document_execution_preserves_values_graphs_and_exact_errors() {
    let fixtures: Vec<Value> =
        serde_json::from_str(include_str!("../../../fixtures/cases.json")).unwrap();
    let extra: Vec<Value> =
        serde_json::from_str(include_str!("../../../fixtures/validation-cases.json")).unwrap();
    for fixture in fixtures.into_iter().chain(extra) {
        for mode in [
            octri_parser::ValidationMode::Compatible,
            octri_parser::ValidationMode::Strict,
        ] {
            for generated in [false, true] {
                for operation in [
                    Operation::Parse,
                    Operation::Bundle,
                    Operation::Dereference,
                    Operation::Validate,
                ] {
                    let mut options: Options = serde_json::from_value(
                        fixture
                            .get("options")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({})),
                    )
                    .unwrap();
                    options.validate.mode = mode;
                    let request = Request {
                        operation,
                        input: Input {
                            uri: "memory://octri/openapi.yaml".into(),
                            content: Some(
                                fixture["input"]
                                    .as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| fixture["input"].to_string()),
                            ),
                            generated,
                        },
                        options,
                    };
                    let full = execute(request.clone(), &mut NoIo);
                    let limits = request.options.limits.clone();
                    let document = octri_parser::execute_document(request, &mut NoIo);
                    match (full, document) {
                        (Ok(full), Ok(document)) => {
                            if let Some(graph) = &document.graph {
                                assert_eq!(
                                    full.document,
                                    graph.to_value(&limits).ok(),
                                    "{} {operation:?}",
                                    fixture["name"]
                                );
                                assert!(document.document.is_none());
                            } else {
                                assert_eq!(
                                    full.document, document.document,
                                    "{} {operation:?}",
                                    fixture["name"]
                                );
                            }
                            assert_eq!(
                                serde_json::to_value(full.graph).unwrap(),
                                serde_json::to_value(document.graph).unwrap()
                            );
                        }
                        (Err(full), Err(document)) => assert_eq!(
                            serde_json::to_value(full).unwrap(),
                            serde_json::to_value(document).unwrap(),
                            "{} {operation:?} {mode:?}",
                            fixture["name"]
                        ),
                        (full, document) => panic!(
                            "{} {operation:?}: full={full:?}, document={document:?}",
                            fixture["name"]
                        ),
                    }
                }
            }
        }
    }
}
