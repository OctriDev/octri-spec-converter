//! Draft 4 uses id, ignored reference siblings and pre-contains schema positions.
use octri_parser::{
    Address, Input, NoIo, Options, SchemaDialect, SchemaRoot, bundle_schema_registry_with_dialects,
};
use serde_json::{Value, json};

fn bundle(payload: Value, files: Vec<(&str, Value)>) -> Result<Value, Box<dyn std::error::Error>> {
    let uri = "memory://legacy/schema.json";
    let mut options = Options::default();
    options.files.insert(uri.into(), payload.to_string());
    for (uri, value) in files {
        options.files.insert(uri.into(), value.to_string());
    }
    Ok(bundle_schema_registry_with_dialects(Input { uri:"memory://generated/api.json".into(),content:Some(json!({"openapi":"3.1.2","info":{"title":"Legacy","version":"1"},"paths":{},"components":{"schemas":{"Payload":{"$ref":uri}}}}).to_string()), generated:true },vec![SchemaRoot {address:Address{uri:uri.into(),pointer:String::new()},dialect:SchemaDialect::Draft4}],vec![],options,&mut NoIo)?.document)
}

fn validator(document: &Value) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let mut registry = jsonschema::Registry::new();
    for schema in document["components"]["schemas"]
        .as_object()
        .ok_or("schemas")?
        .values()
    {
        if let Some(id) = schema
            .get("id")
            .or_else(|| schema.get("$id"))
            .and_then(Value::as_str)
        {
            registry = registry.add(id, jsonschema::Resource::from_contents(schema.clone()))?;
        }
    }
    let registry = registry.prepare()?;
    Ok(jsonschema::options()
        .with_registry(&registry)
        .build(&document["components"]["schemas"]["Payload"])?)
}

#[test]
fn draft4_ids_relative_refs_fragment_ids_tuples_and_exclusive_bounds_survive_relocation()
-> Result<(), Box<dyn std::error::Error>> {
    let document = bundle(
        json!({"$schema":SchemaDialect::Draft4.uri(),"id":"https://schemas.example.test/root.json","definitions":{"Positive":{"id":"#positive","type":"number","minimum":0,"exclusiveMinimum":true},"Nested":{"id":"nested/object.json","type":"object","properties":{"value":{"$ref":"../value.json"}}}},"type":"object","properties":{"positive":{"$ref":"#positive"},"nested":{"$ref":"#/definitions/Nested"},"pair":{"type":"array","items":[{"type":"string"},{"type":"integer"}],"additionalItems":false}},"required":["positive"]}),
        vec![(
            "https://schemas.example.test/value.json",
            json!({"$schema":SchemaDialect::Draft4.uri(),"type":"integer"}),
        )],
    )?;
    let validate = validator(&document)?;
    assert!(validate.is_valid(&json!({"positive":1,"nested":{"value":2},"pair":["x",3]})));
    assert!(!validate.is_valid(&json!({"positive":0})));
    assert!(!validate.is_valid(&json!({"positive":1,"nested":{"value":"wrong"}})));
    assert!(!validate.is_valid(&json!({"positive":1,"pair":["x",3,4]})));
    Ok(())
}

#[test]
fn draft4_unknown_keywords_and_reference_siblings_never_activate_modern_semantics()
-> Result<(), Box<dyn std::error::Error>> {
    let document = bundle(
        json!({"$schema":SchemaDialect::Draft4.uri(),"id":"https://schemas.example.test/inert.json","definitions":{"Text":{"type":"string"}},"type":"object","contains":{"$ref":"missing.json"},"if":{"$ref":"missing.json"},"$defs":{"Fake":{"$ref":"missing.json"}},"properties":{"alias":{"$ref":"#/definitions/Text","id":"missing/base.json","minimum":5,"properties":{"bad":{"$ref":"missing.json"}}}}}),
        vec![],
    )?;
    let report = octri_parser::execute(
        octri_parser::Request {
            operation: octri_parser::Operation::Analyze,
            input: Input {
                uri: "memory://relocated/api.json".into(),
                content: Some(document.to_string()),
                generated: true,
            },
            options: Options::default(),
        },
        &mut NoIo,
    )?;
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.contains("REFERENCE")
                || diagnostic.code.contains("UNRESOLVED")),
        "{:?}",
        report.diagnostics
    );
    let schema = &document["components"]["schemas"]["OctriExternal1"];
    assert_eq!(schema["contains"]["$ref"], "missing.json");
    assert_eq!(schema["$defs"]["Fake"]["$ref"], "missing.json");
    assert!(schema["properties"]["alias"].get("id").is_none());
    assert!(schema["properties"]["alias"].get("properties").is_none());
    Ok(())
}

#[test]
fn draft4_meta_validation_rejects_numeric_exclusive_minimum()
-> Result<(), Box<dyn std::error::Error>> {
    let document = bundle(
        json!({"$schema":SchemaDialect::Draft4.uri(),"type":"number","minimum":0,"exclusiveMinimum":1}),
        vec![],
    )?;
    let mut options = Options::default();
    options.validate.mode = octri_parser::ValidationMode::Strict;
    let report = octri_parser::execute(
        octri_parser::Request {
            operation: octri_parser::Operation::Analyze,
            input: Input {
                uri: "memory://relocated/api.json".into(),
                content: Some(document.to_string()),
                generated: true,
            },
            options,
        },
        &mut NoIo,
    )?;
    assert!(report.diagnostics.iter().any(|d| d.code == "JSON_SCHEMA"));
    Ok(())
}
