//! Imported schemas use their owning API dialect before HTTP projection.
use octri_converter::{ConversionRequest, Status, convert};
use serde_json::{Value, json};

fn request(schema: Value, target: &str) -> Result<ConversionRequest, serde_json::Error> {
    serde_json::from_value(json!({
        "input":{"uri":"memory://imports/events.json","content":json!({
            "asyncapi":"2.6.0","info":{"title":"Events","version":"1"},
            "channels":{"events":{"publish":{"message":{"payload":{
                "$ref":"./api.json#/components/schemas/Value"
            }}}}}
        }).to_string()},
        "files":{"memory://imports/api.json":json!({
            "openapi":"3.0.4","info":{"title":"Models","version":"1"},
            "paths":{},"components":{"schemas":{"Value":schema}}
        }).to_string()},
        "config":{"targetVersion":target,"httpMappings":{"publish events":{
            "method":"POST","path":"/events","server":"https://example.test",
            "responseStatus":"202","security":[],"payloadLocation":"request"
        }}}
    }))
}

fn validator(document: &Value) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let mut registry = jsonschema::Registry::new();
    let mut pending: Vec<_> = document["components"]["schemas"]
        .as_object()
        .ok_or("components")?
        .values()
        .collect();
    while let Some(schema) = pending.pop() {
        if let Some(id) = schema
            .get("$id")
            .or_else(|| schema.get("id"))
            .and_then(Value::as_str)
        {
            registry = registry.add(id, jsonschema::Resource::from_contents(schema.clone()))?;
        }
        for key in ["$defs", "definitions"] {
            if let Some(definitions) = schema.get(key).and_then(Value::as_object) {
                pending.extend(definitions.values());
            }
        }
    }
    let registry = registry
        .add(
            "memory://export/api.json",
            jsonschema::Resource::from_contents(document.clone()),
        )?
        .prepare()?;
    Ok(jsonschema::options().with_registry(&registry).build(&json!({
        "$ref":"memory://export/api.json#/paths/~1events/post/requestBody/content/application~1json/schema"
    }))?)
}

#[test]
fn nullable_and_exclusive_bounds_migrate_with_physical_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    for target in ["3.1.2", "3.2.1"] {
        let input = request(
            json!({"type":"number","nullable":true,"minimum":1,"exclusiveMinimum":true}),
            target,
        )?;
        let report = convert(&input)?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
        let migration = report
            .decisions
            .iter()
            .find(|item| item.code == "EXCLUSIVE_BOUND_MIGRATION")
            .ok_or("migration")?;
        assert_eq!(migration.source.address.uri, "memory://imports/api.json");
        let check = validator(report.document.as_ref().ok_or("output")?)?;
        for accepted in [json!(2), Value::Null] {
            assert!(check.is_valid(&accepted));
        }
        for rejected in [json!(1), json!(0), json!("wrong")] {
            assert!(!check.is_valid(&rejected));
        }
        assert_eq!(
            report
                .manifest
                .files
                .iter()
                .find(|file| file.uri == "memory://imports/api.json")
                .ok_or("source")?
                .bytes,
            input.files["memory://imports/api.json"].len()
        );
    }
    Ok(())
}

#[test]
fn invalid_source_cannot_be_repaired_by_removing_legacy_fields()
-> Result<(), Box<dyn std::error::Error>> {
    for schema in [
        json!({"type":"number","nullable":"wrong"}),
        json!({"type":"number","minimum":1,"exclusiveMinimum":2}),
    ] {
        let error = convert(&request(schema, "3.1.2")?)
            .err()
            .ok_or("invalid legacy source was accepted")?;
        assert_eq!(error.code, "IMPORTED_OPENAPI30_INVALID");
        assert!(
            error
                .diagnostics
                .iter()
                .any(|item| item.location.address.uri == "memory://imports/api.json")
        );
    }
    Ok(())
}

#[test]
fn examples_and_foreign_modern_dialects_stay_literal() -> Result<(), Box<dyn std::error::Error>> {
    let literal = json!({"nullable":true,"exclusiveMinimum":true});
    let mut input = request(
        json!({"type":"object","example":literal,"properties":{
            "count":{"type":"integer","minimum":1,"exclusiveMinimum":true},
            "modern":{"$ref":"./modern.json#/components/schemas/Value"}
        }}),
        "3.2.1",
    )?;
    input.files.insert(
        "memory://imports/modern.json".into(),
        json!({
            "openapi":"3.1.2","info":{"title":"Modern","version":"1"},"paths":{},
            "components":{"schemas":{"Value":{"type":"number","exclusiveMinimum":5}}}
        })
        .to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let document = report.document.as_ref().ok_or("output")?;
    assert!(document.to_string().contains(&literal.to_string()));
    let check = validator(document)?;
    assert!(check.is_valid(&json!({"count":2,"modern":6})));
    assert!(!check.is_valid(&json!({"count":1,"modern":6})));
    assert!(!check.is_valid(&json!({"count":2,"modern":5})));
    Ok(())
}

#[test]
fn plain_external_objects_inherit_legacy_semantics_and_preserve_cycles()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(json!({"$ref":"./types/value.json"}), "3.2.1")?;
    input.files.insert(
        "memory://imports/types/value.json".into(),
        json!({
            "type":"object","properties":{
                "count":{"type":"number","nullable":true,"minimum":1,"exclusiveMinimum":true},
                "next":{"$ref":"./value.json"},
                "other":{"$ref":"./other.json#/Value"}
            }
        })
        .to_string(),
    );
    input.files.insert(
        "memory://imports/types/other.json".into(),
        json!({
            "Value":{"type":"number","minimum":3,"exclusiveMinimum":true}
        })
        .to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let check = validator(report.document.as_ref().ok_or("output")?)?;
    assert!(check.is_valid(&json!({"count":null,"next":{"count":2},"other":4})));
    for value in [
        json!({"count":1}),
        json!({"next":{"count":1}}),
        json!({"other":3}),
    ] {
        assert!(!check.is_valid(&value));
    }
    input.files.insert(
        "memory://imports/types/other.json".into(),
        json!({
            "Value":{"type":"number","nullable":"wrong"}
        })
        .to_string(),
    );
    let error = convert(&input)
        .err()
        .ok_or("invalid external source was accepted")?;
    assert_eq!(error.code, "IMPORTED_OPENAPI30_INVALID");
    assert!(
        error
            .diagnostics
            .iter()
            .any(|item| item.location.address.uri == "memory://imports/types/other.json")
    );
    Ok(())
}

#[test]
fn every_published_target_accepts_migrated_bounds() -> Result<(), Box<dyn std::error::Error>> {
    for target in octri_converter::TARGET_VERSIONS {
        let mut input = request(
            json!({"type":"number","minimum":1,"exclusiveMinimum":true}),
            target,
        )?;
        if *target == "2.0" {
            // Swagger cannot carry an operation-local server. The authored
            // root base URL preserves the same endpoint before this removal.
            input.config.base_url = Some("https://example.test".into());
            input
                .config
                .drop_unsupported
                .insert("/paths/~1events/post/servers".into());
        }
        let report = convert(&input)?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_ne!(
            report.status,
            Status::Blocked,
            "{target}: {:?}",
            report.decisions
        );
        let output = report.document.ok_or("output")?;
        let expanded = octri_parser::execute_document(
            octri_parser::Request {
                operation: octri_parser::Operation::Dereference,
                input: octri_parser::Input {
                    uri: "memory://export/api.json".into(),
                    content: Some(output.to_string()),
                    generated: true,
                },
                options: Default::default(),
            },
            &mut octri_parser::NoIo,
        )?
        .graph
        .ok_or("graph")?
        .to_value(&Default::default())?;
        let schema = expanded
            .pointer(if *target == "2.0" {
                "/paths/~1events/post/parameters/0/schema"
            } else {
                "/paths/~1events/post/requestBody/content/application~1json/schema"
            })
            .ok_or("payload")?;
        let legacy = *target == "2.0" || target.starts_with("3.0.");
        let check = jsonschema::options()
            .with_draft(if legacy {
                jsonschema::Draft::Draft4
            } else {
                jsonschema::Draft::Draft202012
            })
            .build(schema)?;
        assert!(check.is_valid(&json!(2)), "{target}");
        assert!(!check.is_valid(&json!(1)), "{target}");
        assert!(!check.is_valid(&json!("wrong")), "{target}");
    }
    Ok(())
}

#[test]
fn declared_external_dialect_does_not_inherit_legacy_nullable()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(json!({"$ref":"./schema.json"}), "3.2.1")?;
    input.files.insert(
        "memory://imports/schema.json".into(),
        json!({
            "$schema":"http://json-schema.org/draft-04/schema#",
            "type":"number","nullable":true,"minimum":1,"exclusiveMinimum":true
        })
        .to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let check = validator(report.document.as_ref().ok_or("output")?)?;
    assert!(check.is_valid(&json!(2)));
    assert!(!check.is_valid(&json!(1)));
    assert!(!check.is_valid(&Value::Null));
    Ok(())
}

#[test]
fn declared_external_anchor_uses_the_shared_resolved_graph()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(json!({"$ref":"./schema.json#Count"}), "3.2.1")?;
    input.files.insert(
        "memory://imports/schema.json".into(),
        json!({
            "$schema":"https://json-schema.org/draft/2020-12/schema",
            "$id":"memory://imports/schema.json",
            "$defs":{"Count":{"$anchor":"Count","type":"number","exclusiveMinimum":1}}
        })
        .to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let check = validator(report.document.as_ref().ok_or("output")?)?;
    assert!(check.is_valid(&json!(2)));
    assert!(!check.is_valid(&json!(1)));
    Ok(())
}

#[test]
fn ignored_resource_scope_siblings_do_not_rebase_legacy_references()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(json!({"$ref":"./alias.json"}), "3.2.1")?;
    input.files.insert(
        "memory://imports/alias.json".into(),
        json!({
            "$ref":"./number.json","$id":"memory://wrong/"
        })
        .to_string(),
    );
    input.files.insert(
        "memory://imports/number.json".into(),
        json!({"type":"number","minimum":2}).to_string(),
    );
    input.files.insert(
        "memory://wrong/number.json".into(),
        json!({"type":"string"}).to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let check = validator(report.document.as_ref().ok_or("output")?)?;
    assert!(check.is_valid(&json!(2)));
    assert!(!check.is_valid(&json!(1)));
    assert!(!check.is_valid(&json!("wrong retrieval base")));
    assert!(
        report
            .manifest
            .files
            .iter()
            .any(|file| file.uri == "memory://imports/alias.json")
    );
    Ok(())
}

#[test]
fn ignored_owning_api_selectors_do_not_activate_modern_reference_behavior()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(
        json!({"$ref":"./number.json","$id":"memory://wrong/","$schema":"https://json-schema.org/draft/2020-12/schema","$dynamicRef":"missing.json","type":"string"}),
        "3.2.1",
    )?;
    input.files.insert(
        "memory://imports/number.json".into(),
        json!({"type":"number","nullable":true,"minimum":2}).to_string(),
    );
    input.files.insert(
        "memory://wrong/number.json".into(),
        json!({"type":"string"}).to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let check = validator(report.document.as_ref().ok_or("output")?)?;
    assert!(check.is_valid(&json!(2)));
    assert!(check.is_valid(&Value::Null));
    assert!(!check.is_valid(&json!(1)));
    assert!(!check.is_valid(&json!("wrong")));
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "IGNORED_REFERENCE_SIBLINGS"
                && decision.source.address.uri == "memory://imports/api.json")
    );
    Ok(())
}

#[test]
fn nested_declared_json_schema_keeps_modern_identifier_and_constraints()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(json!({"$ref":"./external.json#/Value"}), "3.2.1")?;
    input.files.insert("memory://imports/external.json".into(), json!({"Value":{"$schema":"https://json-schema.org/draft/2020-12/schema","$id":"https://models.example.test/","$ref":"number.json","maximum":8}}).to_string());
    input.files.insert(
        "https://models.example.test/number.json".into(),
        json!({"type":"number","exclusiveMinimum":2}).to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let check = validator(report.document.as_ref().ok_or("output")?)?;
    assert!(check.is_valid(&json!(3)));
    assert!(!check.is_valid(&json!(2)));
    assert!(!check.is_valid(&json!(9)));
    assert!(!check.is_valid(&Value::Null));
    Ok(())
}

#[test]
fn ignored_reference_children_are_not_validated_as_active_schemas()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(
        json!({"$ref":"#/components/schemas/Base","properties":{"ignored":{"nullable":"wrong"}}}),
        "3.2.1",
    )?;
    let mut models: Value = serde_json::from_str(&input.files["memory://imports/api.json"])?;
    models["components"]["schemas"]["Base"] = json!({"type":"number"});
    input
        .files
        .insert("memory://imports/api.json".into(), models.to_string());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let check = validator(report.document.as_ref().ok_or("output")?)?;
    assert!(check.is_valid(&json!(2)));
    assert!(!check.is_valid(&json!("wrong")));
    Ok(())
}

#[test]
fn conflicting_defaults_cannot_silently_share_one_plain_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(json!({"$ref":"./plain.json"}), "3.2.1")?;
    input.files.insert(
        "memory://imports/plain.json".into(),
        json!({"type":"number","nullable":true}).to_string(),
    );
    let mut source: Value = serde_json::from_str(input.input.content.as_ref().ok_or("input")?)?;
    source["channels"]["direct"] =
        json!({"publish":{"message":{"payload":{"$ref":"./plain.json"}}}});
    input.input.content = Some(source.to_string());
    let mut mapping = input.config.http_mappings["publish events"].clone();
    mapping.path = "/direct".into();
    input
        .config
        .http_mappings
        .insert("publish direct".into(), mapping);
    match convert(&input) {
        Err(error) => assert_eq!(error.code, "SCHEMA_DIALECT_CONFLICT"),
        Ok(report) => assert_eq!(
            report.status,
            Status::Blocked,
            "Conflicting defaults must not share one transformed schema"
        ),
    }
    Ok(())
}

#[test]
fn plain_external_reference_siblings_remain_ignored() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(json!({"$ref":"./alias.json"}), "3.2.1")?;
    input.files.insert(
        "memory://imports/alias.json".into(),
        json!({
            "$ref":"./number.json","type":"string","minimum":1000
        })
        .to_string(),
    );
    input.files.insert(
        "memory://imports/number.json".into(),
        json!({
            "type":"number","nullable":true,"minimum":1,"exclusiveMinimum":true
        })
        .to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let check = validator(report.document.as_ref().ok_or("output")?)?;
    assert!(check.is_valid(&json!(2)));
    assert!(check.is_valid(&Value::Null));
    assert!(!check.is_valid(&json!(1)));
    assert!(!check.is_valid(&json!("wrong")));
    assert!(
        report
            .decisions
            .iter()
            .any(|item| item.code == "IGNORED_REFERENCE_SIBLINGS"
                && item.source.address.uri == "memory://imports/alias.json")
    );
    Ok(())
}
