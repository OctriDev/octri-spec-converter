use octri_converter::{Config, ConversionRequest, Status, TARGET_VERSIONS, convert};
use octri_parser::Input;
use serde_json::{Value, json};

fn request(document: Value, target: &str) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://fixtures/api.json".into(),
            content: Some(document.to_string()),
            generated: false,
        },
        files: Default::default(),
        config: Config {
            target_version: Some(target.into()),
            ..Default::default()
        },
    }
}

fn modern() -> Value {
    json!({"openapi":"3.1.2","info":{"title":"Pets","version":"1.0"},"servers":[{"url":"https://api.example.com/v1"}],"paths":{"/pets/{id}":{"get":{"operationId":"getPet","parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"string"}}],"responses":{"200":{"description":"Pet","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Pet"}}}}}}}},"components":{"schemas":{"Pet":{"type":"object","properties":{"id":{"type":"integer","format":"int64","example":18446744073709551615u64}}}},"securitySchemes":{"basic":{"type":"http","scheme":"basic"}}},"security":[{"basic":[]}]})
}

#[test]
fn legacy_rewrites_schema_map_names_without_activating_literal_refs()
-> Result<(), Box<dyn std::error::Error>> {
    let mut source = modern();
    let names = [
        "x-record", "example", "examples", "default", "enum", "const",
    ];
    for name in names {
        source["components"]["schemas"]["Pet"]["properties"][name] =
            json!({"$ref":"#/components/schemas/x-Value"});
    }
    source["components"]["schemas"]["x-Value"] = json!({"type":"string"});
    let literal = json!({"$ref":"#/components/schemas/ThisIsLiteral"});
    source["components"]["schemas"]["Pet"]["example"] = literal.clone();
    source["components"]["schemas"]["Pet"]["x-metadata"] = literal.clone();
    let report = convert(&request(source, "2.0"))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("missing output")?;
    for name in names {
        assert_eq!(
            document["definitions"]["Pet"]["properties"][name]["$ref"],
            "#/definitions/x-Value"
        );
    }
    assert_eq!(document["definitions"]["Pet"]["example"], literal);
    assert_eq!(document["definitions"]["Pet"]["x-metadata"], literal);
    Ok(())
}

#[test]
fn closed_objects_keep_boolean_additional_properties_in_every_target()
-> Result<(), Box<dyn std::error::Error>> {
    let mut source = modern();
    source["components"]["schemas"]["Pet"]["additionalProperties"] = json!(false);
    for target in TARGET_VERSIONS {
        let report = convert(&request(source.clone(), target))?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_eq!(
            report.status,
            Status::Complete,
            "{target}: {:?}",
            report.decisions
        );
        let pointer = if *target == "2.0" {
            "/definitions/Pet/additionalProperties"
        } else {
            "/components/schemas/Pet/additionalProperties"
        };
        assert_eq!(
            report
                .document
                .as_ref()
                .and_then(|document| document.pointer(pointer)),
            Some(&json!(false))
        );
    }
    Ok(())
}

#[test]
fn boolean_schemas_preserve_validation_in_legacy_targets() -> Result<(), Box<dyn std::error::Error>>
{
    for target in ["2.0", "3.0.4"] {
        for boolean in [false, true] {
            let mut source = modern();
            source["components"]["schemas"]["Pet"] = json!(boolean);
            let report = convert(&request(source, target))?;
            assert!(report.valid, "{target}: {:?}", report.diagnostics);
            assert_eq!(
                report.status,
                Status::Complete,
                "{target}: {:?}",
                report.decisions
            );
            let document = report.document.ok_or("missing document")?;
            let schema = if target == "2.0" {
                &document["definitions"]["Pet"]
            } else {
                &document["components"]["schemas"]["Pet"]
            };
            let validator = jsonschema::draft4::new(schema)?;
            for instance in [
                json!(null),
                json!(false),
                json!(0),
                json!(""),
                json!([]),
                json!({}),
            ] {
                assert_eq!(
                    validator.is_valid(&instance),
                    boolean,
                    "{target}: {instance}"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn all_published_targets_validate_and_keep_exact_integer() -> Result<(), Box<dyn std::error::Error>>
{
    for target in TARGET_VERSIONS {
        let report = convert(&request(modern(), target))?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_eq!(
            report.status,
            Status::Complete,
            "{target}: {:?}",
            report.decisions
        );
        let document = report.document.ok_or("missing document")?;
        let pointer = if *target == "2.0" {
            "/definitions/Pet/properties/id/example"
        } else {
            "/components/schemas/Pet/properties/id/example"
        };
        assert_eq!(
            document.pointer(pointer),
            Some(&json!(18446744073709551615u64))
        );
        assert_eq!(
            document[if *target == "2.0" {
                "swagger"
            } else {
                "openapi"
            }],
            *target
        );
    }
    Ok(())
}

#[test]
fn swagger_response_ranges_expand_without_overwriting_explicit_codes()
-> Result<(), Box<dyn std::error::Error>> {
    let source = json!({"openapi":"3.1.2","info":{"title":"Ranges","version":"1"},"servers":[{"url":"https://api.example.com"}],"paths":{"/range":{"get":{"responses":{"2XX":{"description":"Success","content":{"application/json":{"schema":{"type":"string"}}}},"204":{"description":"No content"}}}}}});
    let report = convert(&request(source.clone(), "2.0"))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("missing output")?;
    let responses = document
        .pointer("/paths/~1range/get/responses")
        .and_then(Value::as_object)
        .ok_or("missing responses")?;
    assert_eq!(responses.len(), 100);
    assert_eq!(responses["204"], json!({"description":"No content"}));
    assert_eq!(responses["201"]["schema"], json!({"type":"string"}));
    assert!(!responses.contains_key("2XX"));
    let mut bounded = request(source, "2.0");
    bounded.config.limits.max_output_bytes = 500;
    assert!(convert(&bounded).is_err());
    Ok(())
}

#[test]
fn swagger_auth_and_payload_semantics_survive() -> Result<(), Box<dyn std::error::Error>> {
    let source = json!({"swagger":"2.0","info":{"title":"Create","version":"1"},"host":"api.example.com","basePath":"/v1","schemes":["https"],"consumes":["application/json"],"produces":["application/json"],"securityDefinitions":{"key":{"type":"apiKey","in":"header","name":"X-Key"},"basic":{"type":"basic"}},"security":[{"key":[],"basic":[]},{}],"definitions":{"Pet":{"type":"object","properties":{"name":{"type":"string","x-nullable":true},"age":{"type":"integer","minimum":0,"exclusiveMinimum":true}}}},"paths":{"/pets":{"post":{"parameters":[{"in":"body","name":"pet","required":true,"schema":{"$ref":"#/definitions/Pet"}}],"responses":{"201":{"description":"Created","schema":{"$ref":"#/definitions/Pet"},"examples":{"application/json":{"$ref":"literal-payload","age":18446744073709551615u64}}}}}}}});
    let report = convert(&request(source.clone(), "3.2.1"))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let output = report.document.ok_or("missing output")?;
    assert_eq!(output["security"], source["security"]);
    assert_eq!(
        output.pointer("/paths/~1pets/post/requestBody/content/application~1json/schema/$ref"),
        Some(&json!("#/components/schemas/Pet"))
    );
    assert_eq!(
        output.pointer("/paths/~1pets/post/responses/201/content/application~1json/example"),
        source.pointer("/paths/~1pets/post/responses/201/examples/application~1json")
    );
    assert_eq!(
        output.pointer("/components/schemas/Pet/properties/name/type"),
        Some(&json!(["string", "null"]))
    );
    assert_eq!(
        output.pointer("/components/schemas/Pet/properties/age/exclusiveMinimum"),
        Some(&json!(0))
    );
    Ok(())
}

#[test]
fn loss_requires_individual_explicit_review() -> Result<(), Box<dyn std::error::Error>> {
    let mut source = modern();
    source["components"]["schemas"]["Pet"]["unevaluatedProperties"] = json!(false);
    let mut input = request(source, "3.0.4");
    let blocked = convert(&input)?;
    assert_eq!(blocked.status, Status::Blocked);
    assert!(!blocked.handoff_allowed);
    input.config.accepted_decisions = blocked
        .decisions
        .iter()
        .map(|item| item.id.clone())
        .collect();
    assert_eq!(
        convert(&input)?.status,
        Status::Blocked,
        "Acknowledgement must never clear a blocker"
    );
    input
        .config
        .drop_unsupported
        .insert("/components/schemas/Pet/unevaluatedProperties".into());
    let review = convert(&input)?;
    assert_eq!(review.status, Status::NeedsReview);
    assert!(!review.handoff_allowed);
    input
        .config
        .accepted_decisions
        .extend(review.decisions.iter().map(|item| item.id.clone()));
    let complete = convert(&input)?;
    assert_eq!(complete.status, Status::Complete);
    assert!(complete.handoff_allowed);
    Ok(())
}

#[test]
fn invalid_source_never_becomes_clean_output() {
    assert!(
        convert(&request(
            json!({"swagger":"2.0","info":{"title":"Broken"},"paths":{}}),
            "3.1.2"
        ))
        .is_err()
    );
}
