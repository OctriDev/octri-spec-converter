//! Serialized Postman annotations retain schema scope and original evidence.
use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::{Value, json};

fn request(schema: Value, example: Value) -> ConversionRequest {
    let source = json!({"$kind":"collection","name":"Scoped schemas","auth":{"type":"noauth"},"items":[{
        "$kind":"http-request","name":"Create","method":"POST","url":"https://api.example.test/items",
        "body":{"type":"json","content":example.to_string(),"$type":schema.to_string()},
        "examples":[{"$kind":"http-example","name":"Created","response":{"statusCode":204,"statusText":"Empty"}}]
    }]});
    ConversionRequest {
        input: Input {
            uri: "memory://postman/collection.json".into(),
            content: Some(source.to_string()),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    }
}

fn validator(
    document: &Value,
    pointer: &str,
) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let mut registry = jsonschema::Registry::new();
    for schema in document["components"]["schemas"]
        .as_object()
        .ok_or("schemas")?
        .values()
    {
        if let Some(id) = schema
            .get("$id")
            .or_else(|| schema.get("id"))
            .and_then(Value::as_str)
        {
            registry = registry.add(id, jsonschema::Resource::from_contents(schema.clone()))?;
        }
    }
    let registry = registry
        .add(
            "memory://relocated/api.json",
            jsonschema::Resource::from_contents(document.clone()),
        )?
        .prepare()?;
    Ok(jsonschema::options()
        .with_registry(&registry)
        .build(&json!({"$ref":format!("memory://relocated/api.json#{pointer}")}))?)
}

const BODY: &str = "/paths/~1items/post/requestBody/content/application~1json/schema";

#[test]
fn unscoped_invalid_serialized_constraints_block_legacy_handoff()
-> Result<(), Box<dyn std::error::Error>> {
    for target in ["2.0", "3.0.4"] {
        let mut input = request(
            json!({"type":"number","minimum":2,"exclusiveMinimum":false}),
            json!(3),
        );
        input.config.target_version = Some(target.into());
        let report = convert(&input)?;
        assert_eq!(report.status, Status::Blocked);
        assert!(!report.handoff_allowed);
        assert!(
            report
                .decisions
                .iter()
                .any(|decision| decision.code == "SCHEMA_SOURCE_INVALID")
        );
    }
    Ok(())
}

#[test]
fn malformed_request_and_response_schema_selectors_block_legacy_emission()
-> Result<(), Box<dyn std::error::Error>> {
    for target in ["2.0", "3.0.4"] {
        for response in [false, true] {
            for schema in [
                json!({"$id":"https://schemas.example.test/body.json","$schema":3,"type":"integer"}),
                json!({"$id":"https://schemas.example.test/body.json","type":"object","properties":{"value":{"$schema":3,"type":"integer"}}}),
            ] {
                let mut input = request(schema.clone(), json!({"value":1}));
                input.config.target_version = Some(target.into());
                if response {
                    let mut source: Value =
                        serde_json::from_str(input.input.content.as_deref().ok_or("source")?)?;
                    source["items"][0]
                        .as_object_mut()
                        .ok_or("request")?
                        .remove("body");
                    source["items"][0]["examples"][0]["response"] = json!({"statusCode":200,"statusText":"OK","body":{"type":"json","content":"{\"value\":1}","$type":schema.to_string()}});
                    input.input.content = Some(source.to_string());
                }
                let report = convert(&input)?;
                assert_eq!(
                    report.status,
                    Status::Blocked,
                    "{target} response={response}"
                );
                assert!(!report.handoff_allowed);
                assert!(
                    !report
                        .decisions
                        .iter()
                        .any(|decision| decision.code == "SCHEMA_RESOURCE_LOCALIZED")
                );
                assert!(!report.valid);
            }
        }
    }
    Ok(())
}

#[test]
fn dialects_keep_local_and_supplied_external_reference_scope()
-> Result<(), Box<dyn std::error::Error>> {
    for (dialect, identifier, definitions) in [
        (
            "http://json-schema.org/draft-04/schema#",
            "id",
            "definitions",
        ),
        (
            "http://json-schema.org/draft-07/schema#",
            "$id",
            "definitions",
        ),
        (
            "https://json-schema.org/draft/2020-12/schema",
            "$id",
            "$defs",
        ),
    ] {
        let mut schema = json!({"$schema":dialect,"type":"object","required":["value","enabled"],"properties":{
            "value":{"$ref":"value.json"},"enabled":{"$ref":format!("#/{definitions}/Enabled")}
        }});
        schema[identifier] = json!("https://schemas.example.test/body.json");
        schema[definitions] = json!({"Enabled":{"type":"boolean"}});
        let mut input = request(schema, json!({"value":2,"enabled":true}));
        input.files.insert(
            "https://schemas.example.test/value.json".into(),
            json!({"type":"integer","minimum":1}).to_string(),
        );
        for target in ["3.1.2", "3.2.1"] {
            input.config.target_version = Some(target.into());
            let report = convert(&input)?;
            assert!(report.valid, "{dialect}: {:?}", report.diagnostics);
            assert_eq!(report.status, Status::NeedsReview, "{:?}", report.decisions);
            assert_eq!(report.manifest.files.len(), 2);
            assert!(!serde_json::to_string(&report.manifest.files)?.contains(".octri-schema-"));
            let check = validator(&report.document.ok_or("output")?, BODY)?;
            assert!(check.is_valid(&json!({"value":1,"enabled":false})));
            assert!(!check.is_valid(&json!({"value":0,"enabled":true})));
            assert!(!check.is_valid(&json!({"value":1,"enabled":"wrong"})));
        }
    }
    Ok(())
}

#[test]
fn relative_references_use_collection_directory_and_examples_stay_inert()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(
        json!({"type":"array","items":{"$ref":"schemas/value.json"},"examples":[{"$ref":"never-fetch.json"}],"custom":{"$ref":"also-never-fetch.json"}}),
        json!([true]),
    );
    input.files.insert(
        "schemas/value.json".into(),
        json!({"type":"boolean"}).to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview, "{:?}", report.decisions);
    assert_eq!(report.manifest.files.len(), 2);
    let check = validator(&report.document.ok_or("output")?, BODY)?;
    assert!(check.is_valid(&json!([false])));
    assert!(!check.is_valid(&json!(["wrong"])));
    Ok(())
}

#[test]
fn legacy_reference_siblings_remain_inactive() -> Result<(), Box<dyn std::error::Error>> {
    for dialect in [
        "http://json-schema.org/draft-04/schema#",
        "http://json-schema.org/draft-07/schema#",
    ] {
        let mut input = request(
            json!({"$schema":dialect,"$ref":"value.json","type":"string","properties":{"ignored":{"$ref":"never-fetch.json"}}}),
            json!(7),
        );
        input
            .files
            .insert("value.json".into(), json!({"type":"integer"}).to_string());
        let report = convert(&input)?;
        assert!(report.valid, "{:?}", report.diagnostics);
        let check = validator(&report.document.ok_or("output")?, BODY)?;
        assert!(check.is_valid(&json!(8)));
        assert!(!check.is_valid(&json!("wrong")));
    }
    Ok(())
}

#[test]
fn parse_and_reference_errors_point_at_original_annotation()
-> Result<(), Box<dyn std::error::Error>> {
    for text in ["{", "{\"$ref\":\"#/missing\"}"] {
        let mut input = request(json!({}), json!({}));
        let mut source: Value =
            serde_json::from_str(input.input.content.as_deref().ok_or("source")?)?;
        source["items"][0]["body"]["$type"] = json!(text);
        input.input.content = Some(serde_json::to_string_pretty(&source)?);
        let error = convert(&input).err().ok_or("invalid annotation accepted")?;
        assert!(!error.diagnostics.is_empty(), "{error:?}");
        for diagnostic in &error.diagnostics {
            assert_eq!(diagnostic.location.address.uri, input.input.uri);
            assert_eq!(diagnostic.location.address.pointer, "/items/0/body/$type");
            assert!(diagnostic.location.line.is_some());
            assert!(diagnostic.location.offset.is_some());
        }
    }
    Ok(())
}

#[test]
fn unsupported_dialect_cannot_be_acknowledged_as_success() -> Result<(), Box<dyn std::error::Error>>
{
    let mut input = request(
        json!({"$schema":"https://dialects.example/custom","type":"object"}),
        json!({}),
    );
    let report = convert(&input)?;
    assert_eq!(report.status, Status::Blocked);
    let decision = report
        .decisions
        .iter()
        .find(|decision| decision.code == "POSTMAN_BODY_TYPE_DIALECT")
        .ok_or("dialect decision")?;
    assert_eq!(decision.source.address.pointer, "/items/0/body/$type");
    input.config.accepted_decisions = report
        .decisions
        .iter()
        .map(|decision| decision.id.clone())
        .collect();
    assert_eq!(convert(&input)?.status, Status::Blocked);
    Ok(())
}

#[test]
fn authored_response_schema_and_exact_numbers_survive() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(json!({"const":u64::MAX}), json!(u64::MAX));
    let mut source: Value = serde_json::from_str(input.input.content.as_deref().ok_or("source")?)?;
    source["items"][0]["examples"][0]["response"] = json!({"statusCode":200,"statusText":"OK","body":{
        "type":"json","content":"[true]","$type":json!({"$defs":{"Flag":{"type":"boolean"}},"type":"array","items":{"$ref":"#/$defs/Flag"}}).to_string()
    }});
    input.input.content = Some(source.to_string());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview, "{:?}", report.decisions);
    let document = report.document.ok_or("output")?;
    assert_eq!(
        document["paths"]["/items"]["post"]["requestBody"]["content"]["application/json"]["example"],
        json!(u64::MAX)
    );
    let check = validator(
        &document,
        "/paths/~1items/post/responses/200/content/application~1json/schema",
    )?;
    assert!(check.is_valid(&json!([false])));
    assert!(!check.is_valid(&json!([0])));
    Ok(())
}

#[test]
fn derived_schema_admission_bounds_nodes_and_documents() -> Result<(), Box<dyn std::error::Error>> {
    for nodes in [false, true] {
        let mut input = request(
            json!({"$schema":"https://json-schema.org/draft/2020-12/schema","enum":(0..400).collect::<Vec<_>>()}),
            json!(1),
        );
        let mut source: Value =
            serde_json::from_str(input.input.content.as_deref().ok_or("source")?)?;
        let mut second = source["items"][0].clone();
        second["url"] = json!("https://api.example.test/second");
        source["items"].as_array_mut().ok_or("items")?.push(second);
        input.input.content = Some(source.to_string());
        if nodes {
            input.config.limits.max_nodes = 600;
        } else {
            input.config.limits.max_documents = 1;
        }
        assert_eq!(
            convert(&input).err().ok_or("derived budget ignored")?.code,
            "SCHEMA_DERIVED_LIMIT"
        );
    }
    Ok(())
}

#[test]
fn annotation_without_body_cannot_bypass_validation() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(json!({"$ref":"value.json"}), json!({}));
    let mut source: Value = serde_json::from_str(input.input.content.as_deref().ok_or("source")?)?;
    source["items"][0]["body"]["type"] = json!("none");
    input.input.content = Some(source.to_string());
    assert_eq!(
        convert(&input)
            .err()
            .ok_or("annotation without body accepted")?
            .code,
        "POSTMAN_BODY_TYPE"
    );
    Ok(())
}

#[test]
fn modern_reference_siblings_and_authored_examples_receive_strict_validation()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(json!({"$ref":"value.json","maximum":5}), json!(2));
    input
        .files
        .insert("value.json".into(), json!({"type":"integer"}).to_string());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let check = validator(&report.document.ok_or("output")?, BODY)?;
    assert!(check.is_valid(&json!(5)));
    assert!(!check.is_valid(&json!(6)));
    let invalid = convert(&request(json!({"type":"integer"}), json!("wrong")))?;
    assert!(
        invalid.valid,
        "structural validation: {:?}",
        invalid.diagnostics
    );
    assert_eq!(invalid.status, Status::NeedsReview);
    assert!(!invalid.handoff_allowed);
    let audit = invalid.audit.ok_or("missing example validation")?;
    assert!(
        audit.findings.iter().any(|finding| matches!(
            finding.rule_id.as_str(),
            "schema-example-valid" | "schema-example-type"
        )),
        "{:?}",
        audit.findings
    );
    Ok(())
}

#[test]
fn scoped_static_schemas_export_all_supported_targets() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(
        json!({"type":"object","required":["value"],"properties":{"value":{"$ref":"value.json"}}}),
        json!({"value":7}),
    );
    input
        .files
        .insert("value.json".into(), json!({"type":"integer"}).to_string());
    for target in octri_converter::TARGET_VERSIONS {
        input.config.target_version = Some((*target).into());
        if target == &"2.0" {
            input.config.drop_unsupported = [
                "/paths/~1items/post/servers".into(),
                "/paths/~1items/post/requestBody/content/application~1json/example".into(),
            ]
            .into();
        } else {
            input.config.drop_unsupported.clear();
        }
        let report = convert(&input)?;
        if target == &"2.0" || target.starts_with("3.0.") {
            assert!(report.valid, "{target}: {:?}", report.diagnostics);
            assert_eq!(report.status, Status::NeedsReview, "{:?}", report.decisions);
            assert!(
                report
                    .decisions
                    .iter()
                    .any(|decision| decision.code == "SCHEMA_RESOURCE_LOCALIZED"),
                "{:?}",
                report.decisions
            );
        } else {
            assert!(report.valid, "{target}: {:?}", report.diagnostics);
            assert_ne!(report.status, Status::Blocked);
        }
        let document = report.document.ok_or("output")?;
        assert_eq!(
            document.get("openapi").or_else(|| document.get("swagger")),
            Some(&json!(target))
        );
    }
    Ok(())
}

#[test]
fn portable_inline_annotations_export_all_supported_targets()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(
        json!({"type":"object","required":["value"],"properties":{"value":{"type":"integer"}}}),
        json!({"value":7}),
    );
    for target in octri_converter::TARGET_VERSIONS {
        input.config.target_version = Some((*target).into());
        if target == &"2.0" {
            // Swagger has no operation server or request-media example field.
            input.config.drop_unsupported = [
                "/paths/~1items/post/servers".into(),
                "/paths/~1items/post/requestBody/content/application~1json/example".into(),
            ]
            .into();
        } else {
            input.config.drop_unsupported.clear();
        }
        let report = convert(&input)?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    }
    Ok(())
}

#[test]
fn literal_reference_keys_do_not_create_schema_resources() -> Result<(), Box<dyn std::error::Error>>
{
    let input = request(
        json!({"type":"object","custom":{"$ref":"inert.json"},"examples":[{"$id":"literal","$ref":"also-inert.json"}]}),
        json!({}),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.manifest.files.len(), 1);
    let document = report.document.ok_or("output")?;
    assert!(document["components"].get("schemas").is_none());
    assert_eq!(
        document.pointer(BODY).ok_or("inline schema")?["type"],
        "object"
    );
    Ok(())
}

#[test]
fn shared_body_handler_finishes_imports_for_legacy_collections()
-> Result<(), Box<dyn std::error::Error>> {
    let source = json!({"info":{"name":"Legacy","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"auth":{"type":"noauth"},"item":[{
        "name":"Create","request":{"method":"POST","url":"https://api.example.test/items","body":{"mode":"raw","raw":"7","options":{"raw":{"language":"json"}},"$type":"{\"$ref\":\"value.json\"}"}}
    }]});
    let mut input = request(json!({}), json!({}));
    input.input.content = Some(source.to_string());
    input
        .files
        .insert("value.json".into(), json!({"type":"integer"}).to_string());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let check = validator(&report.document.ok_or("output")?, BODY)?;
    assert!(check.is_valid(&json!(8)));
    assert!(!check.is_valid(&json!("wrong")));
    Ok(())
}
