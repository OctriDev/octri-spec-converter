//! Static resource boundaries can become local pointers in legacy targets.
use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::{Value, json};

#[test]
fn unscoped_invalid_modern_constraints_cannot_gain_legacy_validity()
-> Result<(), Box<dyn std::error::Error>> {
    for target in ["2.0", "3.0.4"] {
        for constraint in [
            json!({"exclusiveMinimum":false}),
            json!({"exclusiveMaximum":true}),
            json!({"$schema":3}),
        ] {
            let mut input = request(json!({"Value":{"type":"number","minimum":2}}), target);
            for (key, value) in constraint.as_object().ok_or("constraint")? {
                input
                    .config
                    .overrides
                    .insert(format!("/components/schemas/Value/{key}"), value.clone());
            }
            let report = convert(&input)?;
            assert_eq!(
                report.status,
                Status::Blocked,
                "{target}: {:?}",
                report.decisions
            );
            assert!(!report.handoff_allowed);
            let blocker = report
                .decisions
                .iter()
                .find(|decision| decision.code == "SCHEMA_SOURCE_INVALID")
                .ok_or("source validation blocker")?;
            assert!(blocker.message.contains("/components/schemas/Value/"));
            assert!(blocker.message.contains("pre-migration path"));
            // Legacy emission may rename or remove the offending location.
            // Never expose a canonical path as a link into the final document.
            assert!(blocker.output_pointers.is_empty());
            input.config.accepted_decisions = report
                .decisions
                .iter()
                .map(|decision| decision.id.clone())
                .collect();
            assert_eq!(convert(&input)?.status, Status::Blocked);
        }
    }
    Ok(())
}

#[test]
fn malformed_selectors_from_overrides_are_never_erased() -> Result<(), Box<dyn std::error::Error>> {
    for target in ["2.0", "3.0.4"] {
        for pointer in ["/components/schemas/Value/$schema", "/jsonSchemaDialect"] {
            let mut input = request(
                json!({"Value":{"$id":"https://schemas.example.test/value.json","type":"integer"}}),
                target,
            );
            input.config.overrides.insert(pointer.into(), json!(3));
            let report = convert(&input)?;
            assert_eq!(report.status, Status::Blocked, "{target}: {pointer}");
            assert!(!report.handoff_allowed);
            assert!(
                !report
                    .decisions
                    .iter()
                    .any(|decision| decision.code == "SCHEMA_RESOURCE_LOCALIZED")
            );
        }
    }
    Ok(())
}

#[test]
fn malformed_constraints_cannot_be_reinterpreted_as_legacy_shapes()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(
        json!({"Value":{"$id":"https://schemas.example.test/value.json","type":"number"}}),
        "3.0.4",
    );
    // A boolean exclusive bound is valid in Draft4, but not in source 2020-12.
    input.config.overrides.insert(
        "/components/schemas/Value/exclusiveMinimum".into(),
        json!(false),
    );
    let report = convert(&input)?;
    assert_eq!(report.status, Status::Blocked);
    assert!(
        !report
            .decisions
            .iter()
            .any(|decision| decision.code == "SCHEMA_RESOURCE_LOCALIZED")
    );
    Ok(())
}

fn request(schemas: Value, target: &str) -> ConversionRequest {
    ConversionRequest {
        input: Input { uri: "memory://resources/api.json".into(), generated: false,
            content: Some(json!({"openapi":"3.1.2","info":{"title":"Schema resources","version":"1"},"paths":{},"components":{"schemas":schemas}}).to_string()) },
        files: Default::default(),config: Config { target_version:Some(target.into()),..Default::default() }
    }
}

fn validator(
    document: &Value,
    name: &str,
    target: &str,
) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let pointer = if target == "2.0" {
        format!("/definitions/{name}")
    } else {
        format!("/components/schemas/{name}")
    };
    let registry = jsonschema::Registry::new()
        .add(
            "memory://relocated/api.json",
            jsonschema::Resource::from_contents(document.clone()),
        )?
        .prepare()?;
    Ok(jsonschema::options()
        .with_draft(jsonschema::Draft::Draft4)
        .with_registry(&registry)
        .build(&json!({"$ref":format!("memory://relocated/api.json#{pointer}")}))?)
}

#[test]
fn nested_static_ids_and_anchors_keep_recursive_validation()
-> Result<(), Box<dyn std::error::Error>> {
    let schemas = json!({"Tree":{
        "$id":"https://schemas.example.test/tree.json","$anchor":"Tree","type":"object",
        "properties":{"value":{"$id":"value.json","type":"integer","minimum":1},"child":{"$ref":"#Tree"}},
        "required":["value"],"additionalProperties":false
    },"Value":{"$ref":"https://schemas.example.test/value.json","maximum":5}});
    for target in ["2.0", "3.0.0", "3.0.4"] {
        let report = convert(&request(schemas.clone(), target))?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
        assert!(
            report
                .decisions
                .iter()
                .any(|decision| decision.code == "SCHEMA_RESOURCE_LOCALIZED")
        );
        let document = report.document.ok_or("output")?;
        let tree = validator(&document, "Tree", target)?;
        assert!(tree.is_valid(&json!({"value":1,"child":{"value":2}})));
        assert!(!tree.is_valid(&json!({"value":1,"child":{"value":0}})));
        assert!(!tree.is_valid(&json!({"value":1,"extra":true})));
        let value = validator(&document, "Value", target)?;
        assert!(value.is_valid(&json!(5)));
        assert!(!value.is_valid(&json!(6)));
    }
    Ok(())
}

#[test]
fn legacy_reference_siblings_remain_ignored_after_scope_removal()
-> Result<(), Box<dyn std::error::Error>> {
    for (dialect, identifier) in [
        ("http://json-schema.org/draft-04/schema#", "id"),
        ("http://json-schema.org/draft-07/schema#", "$id"),
    ] {
        let mut number = json!({"$schema":dialect,"type":"integer"});
        number[identifier] = json!("https://schemas.example.test/number.json");
        let schemas = json!({"Number":number,"Alias":{"$schema":dialect,"$ref":"https://schemas.example.test/number.json","type":"string","properties":{"ignored":{"$ref":"never-read.json"}}}});
        for target in ["2.0", "3.0.4"] {
            let report = convert(&request(schemas.clone(), target))?;
            assert!(report.valid, "{:?}", report.diagnostics);
            assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
            let check = validator(&report.document.ok_or("output")?, "Alias", target)?;
            assert!(check.is_valid(&json!(7)));
            assert!(!check.is_valid(&json!("wrong")));
        }
    }
    Ok(())
}

#[test]
fn localized_pointers_encode_literal_percent_and_unicode_property_names()
-> Result<(), Box<dyn std::error::Error>> {
    let name = "猫 / %2F~ #?";
    let pointer = format!("/properties/{}", name.replace('~', "~0").replace('/', "~1"));
    let reference = format!(
        "https://schemas.example.test/value.json#{}",
        percent_encoding::utf8_percent_encode(&pointer, percent_encoding::NON_ALPHANUMERIC)
    );
    let schemas = json!({"Object":{"$id":"https://schemas.example.test/value.json","type":"object","properties":{name:{"type":"integer"}}},"Alias":{"$ref":reference}});
    for target in ["2.0", "3.0.4"] {
        let report = convert(&request(schemas.clone(), target))?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
        let document = report.document.ok_or("output")?;
        let check = validator(&document, "Alias", target)?;
        assert!(check.is_valid(&json!(1)));
        assert!(!check.is_valid(&json!("wrong")));
    }
    Ok(())
}

#[test]
fn dynamic_scope_and_custom_dialects_cannot_be_localized() -> Result<(), Box<dyn std::error::Error>>
{
    for schema in [
        json!({"$id":"https://schemas.example.test/dynamic.json","$dynamicAnchor":"node","type":"object","properties":{"child":{"$dynamicRef":"#node"}}}),
        json!({"$id":"https://schemas.example.test/custom.json","$schema":"https://dialects.example/custom","type":"object"}),
    ] {
        let mut input = request(json!({"Value":schema}), "3.0.4");
        let report = convert(&input)?;
        assert_eq!(report.status, Status::Blocked);
        assert!(
            !report
                .decisions
                .iter()
                .any(|decision| decision.code == "SCHEMA_RESOURCE_LOCALIZED")
        );
        assert!(
            report.document.as_ref().ok_or("output")?["components"]["schemas"]["Value"]
                .get("$id")
                .is_some()
        );
        input.config.accepted_decisions = report
            .decisions
            .iter()
            .map(|decision| decision.id.clone())
            .collect();
        assert_eq!(convert(&input)?.status, Status::Blocked);
    }
    Ok(())
}

#[test]
fn unknown_reference_graph_from_overrides_keeps_scope_and_fails_integrity()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(
        json!({"Value":{"$id":"https://schemas.example.test/value.json","type":"object"}}),
        "3.0.4",
    );
    input.config.overrides.insert(
        "/components/schemas/Value/properties".into(),
        json!({"broken":{"$ref":"https://never-fetch.invalid/schema.json"}}),
    );
    let report = convert(&input)?;
    assert!(!report.valid);
    assert_eq!(report.status, Status::Blocked);
    assert!(
        !report
            .decisions
            .iter()
            .any(|decision| decision.code == "SCHEMA_RESOURCE_LOCALIZED")
    );
    assert!(
        report.document.ok_or("output")?["components"]["schemas"]["Value"]
            .get("$id")
            .is_some()
    );
    Ok(())
}

#[test]
fn literal_reference_data_and_identifier_property_names_remain_unchanged()
-> Result<(), Box<dyn std::error::Error>> {
    let evidence = json!({"$id":"literal","$ref":"never-fetch.json"});
    let schema = json!({"$id":"https://schemas.example.test/object.json","type":"object","properties":{"$id":{"type":"string"}},"x-evidence":evidence});
    let report = convert(&request(json!({"Value":schema}), "3.0.4"))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("output")?;
    assert_eq!(
        document["components"]["schemas"]["Value"]["x-evidence"],
        evidence
    );
    let check = validator(&document, "Value", "3.0.4")?;
    assert!(check.is_valid(&json!({"$id":"value"})));
    assert!(!check.is_valid(&json!({"$id":1})));
    Ok(())
}
