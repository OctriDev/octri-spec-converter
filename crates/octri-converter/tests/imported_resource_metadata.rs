//! Generated legacy resource containers must remain portable to modern validators.
use octri_converter::{ConversionRequest, convert};
use serde_json::{Value, json};

#[test]
fn legacy_reference_resource_metadata_migrates_with_its_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let source = json!({
        "asyncapi":"2.6.0","info":{"title":"Events","version":"1"},
        "channels":{"events":{"publish":{"message":{"payload":{
            "$ref":"api.json#/components/schemas/Value"
        }}}}}
    });
    let request: ConversionRequest = serde_json::from_value(json!({
        "input":{"uri":"memory://metadata/events.json","content":source.to_string()},
        "files":{
            "api.json":json!({"openapi":"3.0.4","info":{"title":"Models","version":"1"},"paths":{},"components":{"schemas":{"Value":{"$ref":"alias.json"}}}}).to_string(),
            "alias.json":json!({"$ref":"number.json"}).to_string(),
            "number.json":json!({"type":"number","minimum":2,"nullable":true}).to_string()
        },
        "config":{"targetVersion":"3.2.1","httpMappings":{"publish events":{
            "method":"POST","path":"/events","server":"https://example.test",
            "responseStatus":"202","security":[],"payloadLocation":"request"
        }}}
    }))?;
    let report = convert(&request)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("missing document")?;
    let resources = document["components"]["schemas"]
        .as_object()
        .ok_or("missing resources")?;
    let alias = resources
        .values()
        .find(|schema| {
            schema.get("$id").and_then(Value::as_str) == Some("memory://metadata/alias.json")
        })
        .ok_or("legacy alias resource has no modern identity")?;
    assert_eq!(
        alias["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert!(alias.get("id").is_none());
    Ok(())
}
