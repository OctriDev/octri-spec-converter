use octri_parser::{Input, NoIo, Operation, Options, Request, ValidationMode, execute};
use serde_json::json;

#[test]
fn reused_meta_validator_checks_every_modern_schema_root() -> Result<(), Box<dyn std::error::Error>>
{
    for version in ["3.1.2", "3.2.1"] {
        let mut schemas = serde_json::Map::new();
        for index in 0..50 {
            schemas.insert(
                format!("Valid{index}"),
                json!({"type":"object","properties":{"id":{"type":"integer"}}}),
            );
        }
        schemas.insert("BadType".into(), json!({"type":"not-a-json-type"}));
        schemas.insert("BadRequired".into(), json!({"required":"name"}));
        // Custom vocabularies retain their established unverified status.
        schemas.insert(
            "Custom".into(),
            json!({"$schema":"https://dialects.example/custom","type":"custom-type"}),
        );
        let mut options = Options::default();
        options.validate.mode = ValidationMode::Strict;
        let report = execute(Request {
                operation: Operation::Analyze,
            input: Input {
                uri: "memory://validation/api.json".into(),
                content: Some(json!({"openapi":version,"info":{"title":"Many schema roots","version":"1"},"paths":{},"components":{"schemas":schemas}}).to_string()),
                generated: false,
            },
            options,
        },&mut NoIo)?;
        assert!(!report.valid);
        for name in ["BadType", "BadRequired"] {
            assert!(
                report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "JSON_SCHEMA"
                        && diagnostic
                            .location
                            .address
                            .pointer
                            .starts_with(&format!("/components/schemas/{name}/"))),
                "{version}: {:?}",
                report.diagnostics
            );
        }
        assert!(!report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "JSON_SCHEMA"
                && diagnostic
                    .location
                    .address
                    .pointer
                    .starts_with("/components/schemas/Custom")
        }));
    }
    Ok(())
}
