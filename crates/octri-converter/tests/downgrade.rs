use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::{Value, json};

fn request(schema: Value) -> ConversionRequest {
    ConversionRequest{input:Input{uri:"memory://fixtures/api.json".into(),content:Some(json!({"openapi":"3.1.2","info":{"title":"Schema fidelity","version":"1"},"paths":{},"components":{"schemas":{"Value":schema}}}).to_string()),generated:false},files:Default::default(),config:Config{target_version:Some("3.0.4".into()),..Default::default()}}
}

#[test]
fn const_enum_intersections_remain_exact_in_legacy_schemas()
-> Result<(), Box<dyn std::error::Error>> {
    for (schema, accepted) in [
        (json!({"const":1,"enum":[2],"not":{"type":"string"}}), false),
        (json!({"const":1,"enum":[1.0,2]}), true),
    ] {
        for target in ["2.0", "3.0.4"] {
            let mut input = request(schema.clone());
            input.config.target_version = Some(target.into());
            let report = convert(&input)?;
            assert!(report.valid, "{:?}", report.diagnostics);
            assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
            let doc = report.document.ok_or("missing output")?;
            let schema = if target == "2.0" {
                &doc["definitions"]["Value"]
            } else {
                &doc["components"]["schemas"]["Value"]
            };
            let validator = jsonschema::validator_for(schema)?;
            assert_eq!(validator.is_valid(&json!(1)), accepted);
            assert!(!validator.is_valid(&json!(2)));
        }
    }
    Ok(())
}

#[test]
fn exact_bounds_false_and_union_schemas_have_legacy_representations()
-> Result<(), Box<dyn std::error::Error>> {
    for (source, minimum, exclusive) in [
        (
            json!({"type":"integer","minimum":18446744073709551615u64,"exclusiveMinimum":18446744073709551614u64}),
            json!(18446744073709551615u64),
            false,
        ),
        (
            json!({"type":"integer","minimum":10,"exclusiveMinimum":11}),
            json!(11),
            true,
        ),
        (
            json!({"type":"number","minimum":-0.02,"exclusiveMinimum":-0.01}),
            json!(-0.01),
            true,
        ),
    ] {
        let report = convert(&request(source))?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_eq!(report.status, Status::Complete);
        let doc = report.document.ok_or("missing output")?;
        assert_eq!(
            doc.pointer("/components/schemas/Value/minimum"),
            Some(&minimum)
        );
        assert_eq!(
            doc.pointer("/components/schemas/Value/exclusiveMinimum")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            exclusive
        );
    }
    for source in [
        json!(false),
        json!({"type":"null"}),
        json!({"type":["integer","string","null"]}),
    ] {
        let report = convert(&request(source))?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    }
    Ok(())
}

#[test]
fn api_names_and_literal_examples_do_not_trigger_keyword_downgrades()
-> Result<(), Box<dyn std::error::Error>> {
    let source = json!({"openapi":"3.2.1","info":{"title":"Inert data","version":"1"},"paths":{"/pets":{"get":{"responses":{"200":{"description":"Pet","headers":{"x-cursor":{"schema":{"type":["string","null"]}}},"content":{"application/json":{"schema":{"type":"object","properties":{"query":{"type":"string"}}},"examples":{"query":{"value":{"query":"literal","itemSchema":{"$ref":"literal"},"dataValue":false}}}}}}}}}},"components":{"parameters":{"x-client":{"name":"X-Client","in":"header","schema":{"type":["string","null"]}}}}});
    let mut input = request(json!({}));
    input.input.content = Some(source.to_string());
    input.config.target_version = Some("3.1.2".into());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    input.config.target_version = Some("3.0.4".into());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let doc = report.document.ok_or("missing output")?;
    assert_eq!(
        doc.pointer("/paths/~1pets/get/responses/200/headers/x-cursor/schema/nullable"),
        Some(&json!(true))
    );
    assert_eq!(
        doc.pointer("/components/parameters/x-client/schema/nullable"),
        Some(&json!(true))
    );
    Ok(())
}
