use octri_parser::{
    Address, Input, NoIo, Operation, Options, Request, SchemaDialect, SchemaRoot, ValidationMode,
    bundle_schema_registry_with_dialects, execute,
};
use serde_json::{Value, json};

const DIALECT: &str = "https://json-schema.org/draft/2019-09/schema";
const SOURCE: &str = "memory://events/asyncapi.json";

fn analyze(document: &Value) -> Result<octri_parser::Report, octri_parser::ParserError> {
    let mut options = Options::default();
    options.validate.mode = ValidationMode::Strict;
    execute(
        Request {
            operation: Operation::Analyze,
            input: Input {
                uri: "memory://relocated/openapi.json".into(),
                content: Some(document.to_string()),
                generated: true,
            },
            options,
        },
        &mut NoIo,
    )
}

fn api(schema: Value) -> Value {
    json!({"openapi":"3.1.2","info":{"title":"2019 schemas","version":"1"},"paths":{},
        "components":{"schemas":{"Payload":schema}}})
}

fn bundle(schema: Value) -> Result<octri_parser::BundledSource, octri_parser::ParserError> {
    let mut options = Options::default();
    options
        .files
        .insert(SOURCE.into(), json!({"payload":schema}).to_string());
    bundle_schema_registry_with_dialects(
        Input {
            uri: "memory://generated/api.json".into(),
            content: Some(api(json!({"$ref":format!("{SOURCE}#/payload")})).to_string()),
            generated: true,
        },
        vec![SchemaRoot {
            address: Address {
                uri: SOURCE.into(),
                pointer: "/payload".into(),
            },
            dialect: SchemaDialect::Draft201909,
        }],
        vec![],
        options,
        &mut NoIo,
    )
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
fn recognizes_draft201909_identifiers() {
    for uri in [
        DIALECT,
        "https://json-schema.org/draft/2019-09/schema#",
        "http://json-schema.org/draft/2019-09/schema",
    ] {
        assert_eq!(
            SchemaDialect::from_uri(uri),
            Some(SchemaDialect::Draft201909)
        );
    }
    assert!(!SchemaDialect::Draft201909.ignores_reference_siblings());
}

#[test]
fn tuple_and_dependent_schemas_retain_references_and_constraints()
-> Result<(), Box<dyn std::error::Error>> {
    let result = bundle(json!({"$schema":DIALECT,"type":"object",
        "$defs":{"Text":{"$anchor":"text","type":"string","minLength":2}},
        "properties":{"pair":{"type":"array","items":[{"$ref":"#text"},{"type":"integer"}],"additionalItems":false}},
        "dependentSchemas":{"flag":{"properties":{"code":{"$ref":"#text"}},"required":["code"]}},
        "dependentRequired":{"card":["billing"]},
        "prefixItems":[{"$ref":"not-a-schema.json"}],
        "example":{"$ref":"not-a-schema.json"}}))?;
    let report = analyze(&result.document)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(report.references.iter().all(|edge| edge.resolved));
    assert!(
        result
            .origins
            .values()
            .any(|origin| origin.pointer == "/payload/properties/pair/items/0")
    );
    let check = validator(&result.document)?;
    for value in [
        json!({}),
        json!({"pair":["ok",2]}),
        json!({"flag":true,"code":"ok"}),
        json!({"card":1,"billing":2}),
    ] {
        assert!(check.is_valid(&value), "{value}");
    }
    for value in [
        json!({"pair":["x",2]}),
        json!({"pair":["ok","bad"]}),
        json!({"pair":["ok",2,3]}),
        json!({"flag":true}),
        json!({"card":1}),
    ] {
        assert!(!check.is_valid(&value), "{value}");
    }
    Ok(())
}

#[test]
fn recursive_resource_keeps_exact_fragment_and_validates_descendants()
-> Result<(), Box<dyn std::error::Error>> {
    let result = bundle(
        json!({"$schema":DIALECT,"$id":"https://schemas.example.test/tree",
        "$recursiveAnchor":true,"type":"object","properties":{
            "value":{"type":"integer"},"children":{"type":"array","items":{"$recursiveRef":"#"}}}}),
    )?;
    let report = analyze(&result.document)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let edge = result
        .references
        .iter()
        .find(|edge| edge.kind == "$recursiveRef")
        .ok_or("recursive edge")?;
    assert_eq!(edge.reference, "#");
    assert_eq!(
        edge.target.as_ref().ok_or("target")?.address.pointer,
        "/payload"
    );
    assert!(
        result
            .document
            .to_string()
            .contains("\"$recursiveRef\":\"#\"")
    );
    let check = validator(&result.document)?;
    assert!(check.is_valid(&json!({"value":1,"children":[{"value":2}]})));
    assert!(!check.is_valid(&json!({"children":[{"value":"bad"}]})));
    Ok(())
}

#[test]
fn recursive_extension_keeps_dynamic_scope_after_bundling() -> Result<(), Box<dyn std::error::Error>>
{
    let schema = json!({"$schema":DIALECT,"$id":"https://schemas.example.test/strict-tree",
        "$recursiveAnchor":true,"$ref":"#/$defs/tree","required":["name"],
        "$defs":{"tree":{"$id":"https://schemas.example.test/tree","$recursiveAnchor":true,
            "type":"object","properties":{"name":{"type":"string"},"children":{"type":"array","items":{"$recursiveRef":"#"}}}}}});
    let original = jsonschema::options().build(&schema)?;
    assert!(!original.is_valid(&json!({"name":"root","children":[{}]})));
    let result = bundle(schema)?;
    assert!(analyze(&result.document)?.valid);
    let check = validator(&result.document)?;
    assert!(check.is_valid(&json!({"name":"root","children":[{"name":"child"}]})));
    assert!(
        !check.is_valid(&json!({"name":"root","children":[{}]})),
        "{}",
        serde_json::to_string_pretty(&result.document)?
    );
    Ok(())
}

#[test]
fn recursive_fallback_cannot_reclassify_openapi_root_as_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let report = analyze(&api(json!({"$schema":DIALECT,"type":"object",
        "properties":{"child":{"$recursiveRef":"#"}}})))?;
    assert!(!report.valid);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|item| item.code == "RECURSIVE_REFERENCE_TARGET")
    );
    Ok(())
}

#[test]
fn dialect_specific_reference_keywords_stay_inert_elsewhere()
-> Result<(), Box<dyn std::error::Error>> {
    for (dialect, inactive) in [
        (DIALECT, "$dynamicRef"),
        (SchemaDialect::Draft202012.uri(), "$recursiveRef"),
        (SchemaDialect::Draft7.uri(), "$recursiveRef"),
    ] {
        let report = analyze(&api(
            json!({"$schema":dialect,"type":"string",inactive:"missing.json"}),
        ))?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert!(report.references.is_empty());
    }
    let report = analyze(&api(json!({"$schema":SchemaDialect::Draft202012.uri(),
        "type":"array","additionalItems":{"$ref":"inert.json"}})))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(report.references.is_empty());
    Ok(())
}

#[test]
fn malformed_recursive_keywords_and_unsupported_fragments_fail()
-> Result<(), Box<dyn std::error::Error>> {
    for invalid in [
        json!({"$recursiveAnchor":"true"}),
        json!({"$recursiveRef":42}),
        json!({"$recursiveRef":"#/properties/child"}),
        json!({"$recursiveRef":"other.json#"}),
        json!({"items":[7]}),
        json!({"dependentRequired":{"card":"billing"}}),
    ] {
        let mut schema = invalid;
        schema["$schema"] = json!(DIALECT);
        schema["$id"] = json!("https://schemas.example.test/invalid");
        match analyze(&api(schema)) {
            Ok(report) => assert!(!report.valid, "{:?}", report.diagnostics),
            Err(error) => assert!(!error.diagnostics.is_empty()),
        }
    }
    Ok(())
}
