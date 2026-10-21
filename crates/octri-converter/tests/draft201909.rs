use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::{Value, json};

const DIALECT: &str = "https://json-schema.org/draft/2019-09/schema";

fn request(schema: Value, target: &str) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://fixtures/2019.json".into(),
            content: Some(
                json!({"openapi":"3.1.2","info":{"title":"2019 schemas","version":"1"},
                "paths":{},"components":{"schemas":{"Payload":schema}}})
                .to_string(),
            ),
            generated: false,
        },
        files: Default::default(),
        config: Config {
            target_version: Some(target.into()),
            ..Default::default()
        },
    }
}

fn tuple() -> Value {
    json!({"$schema":DIALECT,"$id":"https://schemas.example.test/pair","type":"array",
        "$defs":{"Code":{"type":"string","minLength":2}},
        "items":[{"$ref":"#/$defs/Code"},{"type":"integer"}],"additionalItems":false,
        "prefixItems":[{"$ref":"inert.json"}]})
}

fn validator(document: &Value) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let mut registry = jsonschema::Registry::new();
    for schema in document["components"]["schemas"]
        .as_object()
        .ok_or("schemas")?
        .values()
    {
        if let Some(id) = schema.get("$id").and_then(Value::as_str) {
            registry = registry.add(id, jsonschema::Resource::from_contents(schema.clone()))?;
        }
    }
    Ok(jsonschema::options()
        .with_registry(&registry.prepare()?)
        .build(&document["components"]["schemas"]["Payload"])?)
}

#[test]
fn every_modern_target_preserves_2019_tuple_semantics() -> Result<(), Box<dyn std::error::Error>> {
    for target in ["3.1.0", "3.1.1", "3.1.2", "3.2.0", "3.2.1"] {
        let report = convert(&request(tuple(), target))?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
        let document = report.document.ok_or("document")?;
        assert_eq!(document["openapi"], target);
        assert_eq!(
            document["components"]["schemas"]["Payload"]["$schema"],
            DIALECT
        );
        let check = validator(&document)?;
        assert!(check.is_valid(&json!(["ok", 2])));
        for invalid in [json!(["x", 2]), json!(["ok", "bad"]), json!(["ok", 2, 3])] {
            assert!(!check.is_valid(&invalid), "{target}: {invalid}");
        }
    }
    Ok(())
}

#[test]
fn recursive_scope_survives_conversion_and_static_target_rewriting()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = json!({"$schema":DIALECT,"$id":"https://schemas.example.test/strict-tree",
        "$recursiveAnchor":true,"$ref":"#/$defs/tree","required":["name"],
        "$defs":{"tree":{"$id":"https://schemas.example.test/tree","$recursiveAnchor":true,
            "type":"object","properties":{"name":{"type":"string"},"children":{"type":"array","items":{"$recursiveRef":"#"}}}}}});
    for target in ["3.1.2", "3.2.1"] {
        let report = convert(&request(schema.clone(), target))?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
        let document = report.document.ok_or("document")?;
        let check = validator(&document)?;
        assert!(check.is_valid(&json!({"name":"root","children":[{"name":"child"}]})));
        assert!(!check.is_valid(&json!({"name":"root","children":[{}]})));
        assert_eq!(
            document["components"]["schemas"]["Payload"]["$defs"]["tree"]["properties"]["children"]
                ["items"]["$recursiveRef"],
            "#"
        );
    }
    Ok(())
}

#[test]
fn incompatible_legacy_targets_require_replacement_or_explicit_loss()
-> Result<(), Box<dyn std::error::Error>> {
    for target in ["2.0", "3.0.0", "3.0.1", "3.0.2", "3.0.3", "3.0.4"] {
        let mut input = request(tuple(), target);
        let report = convert(&input)?;
        assert_eq!(
            report.status,
            Status::Blocked,
            "{target}: {:?}",
            report.decisions
        );
        input.config.accepted_decisions = report
            .decisions
            .iter()
            .map(|decision| decision.id.clone())
            .collect();
        assert_eq!(convert(&input)?.status, Status::Blocked);
        assert!(
            report.decisions.iter().any(|decision| decision
                .source
                .address
                .pointer
                .ends_with("/items"))
        );
    }
    Ok(())
}

#[test]
fn malformed_2019_constraints_fail_before_legacy_migration()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = json!({"$schema":DIALECT,"items":[7]});
    let mut input = request(schema, "3.0.4");
    let error = convert(&input).err().ok_or("malformed source accepted")?;
    assert_eq!(error.code, "SOURCE_INVALID");
    let diagnostic = error
        .diagnostics
        .iter()
        .find(|item| item.code == "JSON_SCHEMA")
        .ok_or("schema diagnostic")?;
    assert_eq!(
        diagnostic.location.address.pointer,
        "/components/schemas/Payload/items"
    );
    assert!(diagnostic.location.offset.is_some());
    input.config.bundle = false;
    let error = convert(&input)
        .err()
        .ok_or("malformed unbundled source accepted")?;
    assert_eq!(error.code, "SOURCE_INVALID");
    assert!(
        error
            .diagnostics
            .iter()
            .any(|item| item.code == "JSON_SCHEMA")
    );
    Ok(())
}
