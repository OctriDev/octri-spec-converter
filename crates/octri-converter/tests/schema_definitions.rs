//! Reusable schema definitions migrate losslessly after static scope resolution.
use octri_converter::{ConversionRequest, Status, convert};
use serde_json::{Value, json};

fn request(schema: Value, target: &str) -> Result<ConversionRequest, serde_json::Error> {
    serde_json::from_value(
        json!({"input":{"uri":"memory://input/api.json","content":json!({
        "openapi":"3.1.2","info":{"title":"Definitions","version":"1"},"paths":{},
        "components":{"schemas":{"Root":schema}}
    }).to_string()},"config":{"targetVersion":target}}),
    )
}

fn validator(
    document: &Value,
    name: &str,
) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let base = "memory://output/api.json";
    let mut validation = document.clone();
    validation["$schema"] = json!("http://json-schema.org/draft-04/schema#");
    let registry = jsonschema::Registry::new()
        .add(base, jsonschema::Resource::from_contents(validation))?
        .prepare()?;
    let path = if document.get("swagger").is_some() {
        "definitions"
    } else {
        "components/schemas"
    };
    Ok(jsonschema::options()
        .with_registry(&registry)
        .build(&json!({"$ref":format!("{base}#/{path}/{name}")}))?)
}

#[test]
fn unscoped_recursive_nested_definitions_and_literal_data_keep_meaning()
-> Result<(), Box<dyn std::error::Error>> {
    let literal = json!({"$defs":{"literal":{"$ref":"inert.json"}}});
    for target in ["2.0", "3.0.4"] {
        let schema = json!({"type":"object","properties":{"node":{"$ref":"#/components/schemas/Root/$defs/Node"}},"required":["node"],
            "$defs":{"Node":{"type":"object","properties":{"count":{"$ref":"#/components/schemas/Root/$defs/Node/$defs/Count"},"next":{"$ref":"#/components/schemas/Root/$defs/Node"}},"required":["count"],"$defs":{"Count":{"type":"integer","minimum":2}}}},"example":literal});
        let report = convert(&request(schema, target)?)?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
        let document = report.document.ok_or("output")?;
        let path = if target == "2.0" {
            "/definitions/Root/example"
        } else {
            "/components/schemas/Root/example"
        };
        assert_eq!(document.pointer(path), Some(&literal));
        let check = validator(&document, "Root")?;
        assert!(check.is_valid(&json!({"node":{"count":2,"next":{"count":3}}})));
        assert!(!check.is_valid(&json!({"node":{"count":2,"next":{"count":1}}})));
        assert!(!check.is_valid(&json!({"node":{"count":"wrong"}})));
    }
    Ok(())
}

#[test]
fn hoisted_constraints_retain_legacy_dialect_and_escaped_names()
-> Result<(), Box<dyn std::error::Error>> {
    for target in ["2.0", "3.0.4"] {
        let schema = json!({"$schema":"http://json-schema.org/draft-04/schema#","definitions":{"猫/~%":{"type":"number","minimum":2,"exclusiveMinimum":true}},"$ref":"#/components/schemas/Root/definitions/%E7%8C%AB~1~0%25"});
        let mut input = request(schema, target)?;
        let content = input.input.content.as_ref().ok_or("source")?;
        let mut document: Value = serde_json::from_str(content)?;
        document["components"]["schemas"]["Selected"] =
            json!({"$ref":"#/components/schemas/Root/definitions/%E7%8C%AB~1~0%25"});
        input.input.content = Some(document.to_string());
        let report = convert(&input)?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
        let check = validator(&report.document.ok_or("output")?, "Selected")?;
        assert!(check.is_valid(&json!(3)));
        assert!(!check.is_valid(&json!(2)));
        assert!(!check.is_valid(&json!("wrong")));
    }
    Ok(())
}

#[test]
fn invalid_dynamic_or_map_selected_resources_never_gain_hoisted_validity()
-> Result<(), Box<dyn std::error::Error>> {
    for schema in [
        json!({"$defs":{"Bad":{"minimum":"wrong"}}}),
        json!({"$dynamicAnchor":"node","$defs":{"Child":{"$dynamicRef":"#node"}}}),
        json!({"$defs":{"not":{}},"$ref":"#/components/schemas/Root/$defs"}),
    ] {
        let report = match convert(&request(schema, "3.0.4")?) {
            Ok(report) => report,
            Err(error) => {
                assert_eq!(error.code, "SOURCE_INVALID");
                continue;
            }
        };
        assert_eq!(report.status, Status::Blocked);
        assert!(
            !report
                .decisions
                .iter()
                .any(|decision| decision.code == "SCHEMA_DEFINITION_HOISTED")
        );
    }
    Ok(())
}

#[test]
fn unused_empty_definition_maps_need_no_loss_review() -> Result<(), Box<dyn std::error::Error>> {
    for target in ["2.0", "3.0.0", "3.0.1", "3.0.2", "3.0.3", "3.0.4"] {
        for dialect in [
            "http://json-schema.org/draft-04/schema#",
            "http://json-schema.org/draft-07/schema#",
            "https://json-schema.org/draft/2019-09/schema",
            "https://json-schema.org/draft/2020-12/schema",
        ] {
            let keyword = if dialect.contains("draft-0") {
                "definitions"
            } else {
                "$defs"
            };
            let mut schema = json!({"$schema":dialect,"type":"object","properties":{"count":{"type":"integer","minimum":2}},"required":["count"],"example":{"$defs":{},"definitions":{}}});
            schema[keyword] = json!({});
            schema["properties"]["count"][keyword] = json!({});
            let report = convert(&request(schema, target)?)?;
            assert!(report.valid, "{target} {dialect}: {:?}", report.diagnostics);
            assert_ne!(
                report.status,
                Status::Blocked,
                "{target} {dialect}: {:?}",
                report.decisions
            );
            assert_eq!(
                report
                    .decisions
                    .iter()
                    .filter(|decision| decision.code == "EMPTY_SCHEMA_DEFINITIONS_REMOVED")
                    .count(),
                2
            );
            let document = report.document.ok_or("output")?;
            let path = if target == "2.0" {
                "/definitions/Root"
            } else {
                "/components/schemas/Root"
            };
            let output = document.pointer(path).ok_or("root")?;
            assert!(output.get(keyword).is_none());
            assert_eq!(output["example"], json!({"$defs":{},"definitions":{}}));
            let check = validator(&document, "Root")?;
            assert!(check.is_valid(&json!({"count":2})));
            assert!(!check.is_valid(&json!({"count":1})));
            assert!(!check.is_valid(&json!({})));
        }
    }
    Ok(())
}

#[test]
fn referenced_empty_maps_are_not_removed_as_unused() -> Result<(), Box<dyn std::error::Error>> {
    let report = convert(&request(
        json!({"$defs":{},"$ref":"#/components/schemas/Root/$defs"}),
        "3.0.4",
    )?)?;
    assert_eq!(report.status, Status::Blocked);
    assert!(
        !report
            .decisions
            .iter()
            .any(|decision| decision.code == "EMPTY_SCHEMA_DEFINITIONS_REMOVED")
    );
    Ok(())
}

#[test]
fn modern_targets_retain_empty_reserved_locations() -> Result<(), Box<dyn std::error::Error>> {
    for target in ["3.1.0", "3.1.1", "3.1.2", "3.2.0", "3.2.1"] {
        let schema = json!({"type":"integer","$defs":{},"definitions":{}});
        let report = convert(&request(schema.clone(), target)?)?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_eq!(
            report.document.ok_or("output")?["components"]["schemas"]["Root"],
            schema
        );
        assert!(
            !report
                .decisions
                .iter()
                .any(|decision| decision.code == "EMPTY_SCHEMA_DEFINITIONS_REMOVED")
        );
    }
    Ok(())
}

#[test]
fn ignored_legacy_reference_siblings_are_not_active_definition_maps()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(
        json!({"$schema":"http://json-schema.org/draft-07/schema#","$ref":"#/components/schemas/Text","definitions":{}}),
        "3.0.4",
    )?;
    let mut source: Value = serde_json::from_str(input.input.content.as_deref().ok_or("source")?)?;
    source["components"]["schemas"]["Text"] = json!({"type":"string"});
    input.input.content = Some(source.to_string());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    assert!(
        !report
            .decisions
            .iter()
            .any(|decision| decision.code == "EMPTY_SCHEMA_DEFINITIONS_REMOVED")
    );
    let check = validator(&report.document.ok_or("output")?, "Root")?;
    assert!(check.is_valid(&json!("text")));
    assert!(!check.is_valid(&json!(2)));
    Ok(())
}
