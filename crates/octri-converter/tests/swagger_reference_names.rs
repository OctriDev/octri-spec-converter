use octri_converter::{ConversionRequest, convert};
use serde_json::json;

#[test]
fn schema_and_response_names_do_not_hide_references_or_activate_literals()
-> Result<(), Box<dyn std::error::Error>> {
    let source = json!({
        "swagger":"2.0", "info":{"title":"Names", "version":"1"},
        "paths":{"/values":{"get":{"responses":{"200":{"$ref":"#/responses/example"}}}}},
        "definitions":{
            "Value":{"type":"string"},
            "example":{"type":"object","properties":{
                "default":{"$ref":"#/definitions/Value"},
                "example":{"$ref":"#/definitions/Value"},
                "enum":{"$ref":"#/definitions/Value"},
                "x-record":{"$ref":"#/definitions/Value"}
            },"example":{"$ref":"#/definitions/literal-not-a-reference"}}
        },
        "responses":{"example":{"description":"Value","schema":{"$ref":"#/definitions/example"}}},
        "x-literal":{"$ref":"#/definitions/also-inert"}
    });
    let request: ConversionRequest = serde_json::from_value(
        json!({"input":{"uri":"memory://names/swagger.json","content":source.to_string()},"config":{"targetVersion":"3.1.2"}}),
    )?;
    let report = convert(&request)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let doc = report.document.ok_or("missing document")?;
    for name in ["default", "example", "enum", "x-record"] {
        assert_eq!(
            doc["components"]["schemas"]["example"]["properties"][name]["$ref"],
            "#/components/schemas/Value"
        );
    }
    assert_eq!(
        doc["components"]["schemas"]["example"]["example"]["$ref"],
        "#/definitions/literal-not-a-reference"
    );
    assert_eq!(doc["x-literal"]["$ref"], "#/definitions/also-inert");
    assert_eq!(
        doc["components"]["responses"]["example"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/example"
    );
    Ok(())
}
