use octri_parser::{Input, NoIo, Operation, Options, Request, execute};
use serde_json::json;

#[test]
fn swagger_response_examples_are_literal_in_every_operation()
-> Result<(), Box<dyn std::error::Error>> {
    let document = json!({"swagger":"2.0","info":{"title":"Examples","version":"1"},"paths":{"/pets":{"get":{"responses":{"200":{"description":"Pet","examples":{"application/json":{"$ref":"https://never-fetch.example/payload","value":{"$ref":"literal"}}}}}}}}});
    for operation in [
        Operation::Analyze,
        Operation::Bundle,
        Operation::Dereference,
        Operation::Validate,
    ] {
        let report = execute(
            Request {
                operation,
                input: Input {
                    uri: "memory://test/api.json".into(),
                    content: Some(document.to_string()),
                    generated: false,
                },
                options: Options::default(),
            },
            &mut NoIo,
        )?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert!(report.references.is_empty());
        let output = if let Some(document) = report.document {
            document
        } else {
            report
                .graph
                .ok_or("missing graph")?
                .to_value(&Options::default().limits)?
        };
        assert_eq!(
            output.pointer("/paths/~1pets/get/responses/200/examples"),
            document.pointer("/paths/~1pets/get/responses/200/examples")
        );
    }
    Ok(())
}
