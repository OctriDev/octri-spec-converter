//! Inlining a selected schema preserves dialect and reference conjunctions.
use octri_parser::{Input, NoIo, Operation, Options, Request, bundle_sources, execute};
use serde_json::{Value, json};

fn check(schemas: Value) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let document = bundle_sources(Input {
        uri: "memory://input/api.json".into(), generated: false,
        content: Some(json!({"openapi":"3.1.2","info":{"title":"Inlining","version":"1"},"paths":{},"components":{"schemas":schemas}}).to_string()),
    }, Options::default(), &mut NoIo)?.document;
    let report = execute(
        Request {
            operation: Operation::Analyze,
            input: Input {
                uri: "memory://relocated/api.json".into(),
                generated: true,
                content: Some(document.to_string()),
            },
            options: Options::default(),
        },
        &mut NoIo,
    )?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let mut registry = jsonschema::Registry::new();
    for (name, schema) in document["components"]["schemas"]
        .as_object()
        .ok_or("schemas")?
    {
        registry = registry.add(
            format!("memory://component/{name}"),
            jsonschema::Resource::from_contents(schema.clone()),
        )?;
    }
    let registry = registry
        .add(
            "memory://output/api.json",
            jsonschema::Resource::from_contents(document.clone()),
        )?
        .prepare()?;
    Ok(jsonschema::options()
        .with_registry(&registry)
        .build(&document["components"]["schemas"]["Payload"])?)
}

#[test]
fn referenced_ignored_sibling_keeps_its_inherited_draft4_bounds()
-> Result<(), Box<dyn std::error::Error>> {
    let validate = check(json!({"Payload":{
        "$schema":"http://json-schema.org/draft-04/schema#",
        "$ref":"#/components/schemas/Payload/definitions/Count",
        "definitions":{"Count":{"type":"number","minimum":0,"exclusiveMinimum":true}}
    }}))?;
    assert!(validate.is_valid(&json!(1)));
    assert!(!validate.is_valid(&json!(0)));
    assert!(!validate.is_valid(&json!("wrong")));
    Ok(())
}

#[test]
fn modern_reference_siblings_remain_conjuncts_when_target_needs_inlining()
-> Result<(), Box<dyn std::error::Error>> {
    let validate = check(json!({
        "Payload":{"$ref":"#/components/schemas/Z/definitions/Count","minimum":5},
        "Z":{"$schema":"http://json-schema.org/draft-04/schema#",
            "$ref":"#/components/schemas/Z/definitions/Text",
            "definitions":{"Count":{"type":"number","minimum":0,"exclusiveMinimum":true},"Text":{"type":"string"}}}
    }))?;
    assert!(validate.is_valid(&json!(5)));
    assert!(!validate.is_valid(&json!(4)));
    assert!(!validate.is_valid(&json!(0)));
    assert!(!validate.is_valid(&json!("wrong")));
    Ok(())
}
