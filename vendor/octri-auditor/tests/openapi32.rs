#![allow(clippy::unwrap_used)]
use octri_auditor::{AuditRequest, audit, fixes};
use serde_json::{Value, json};

fn source(version: &str) -> Value {
    json!({"openapi":version,"info":{"title":"New API","version":"1.0.0"},"servers":[{"url":"https://api.example.com","name":"production"}],"security":[],"tags":[{"name":"pets","kind":"nav"}],"paths":{"/pets":{"get":{"operationId":"listPets","summary":"List pets","tags":["pets"],"responses":{"200":{"description":"Pets","content":{"application/json":{"schema":{"type":"string"},"examples":{"pet":{"dataValue":"Ada"}}}}},"400":{"description":"Bad request"}}}}}})
}
fn request(value: Value) -> AuditRequest {
    serde_json::from_value(
        json!({"input":{"uri":"memory://audit/openapi.json","content":value.to_string()}}),
    )
    .unwrap()
}

#[test]
fn all_current_openapi_families_preserve_their_version() {
    for version in ["3.0.4", "3.1.2", "3.2.0", "3.2.1"] {
        let mut value = source(version);
        if version != "3.2.0" && version != "3.2.1" {
            value["servers"][0].as_object_mut().unwrap().remove("name");
            value["tags"][0].as_object_mut().unwrap().remove("kind");
            value["paths"]["/pets"]["get"]["responses"]["200"]["content"]["application/json"]["examples"]
                ["pet"] = json!({"value":"Ada"});
        }
        let report = audit(&request(value.clone())).unwrap();
        assert!(
            report.valid && report.complete,
            "{version}: {:?}",
            report.diagnostics
        );
        let bundled = octri_auditor::bundle(&request(value)).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(bundled.input.content.as_ref().unwrap()).unwrap()["openapi"],
            version
        );
    }
}

#[test]
fn query_additional_operations_and_inherited_parameter_fixes_keep_physical_sources() {
    let mut value = source("3.2.1");
    let operation = value["paths"]["/pets"]["get"].take();
    value["paths"] = json!({"/pets/{id}":{"parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"string"}}],"query":operation,"additionalOperations":{"COPY":operation}}});
    value["paths"]["/pets/{id}"]["query"]["operationId"] = json!("queryPets");
    value["paths"]["/pets/{id}"]["additionalOperations"]["COPY"]
        .as_object_mut()
        .unwrap()
        .remove("operationId");
    let request = request(value);
    let report = audit(&request).unwrap();
    assert!(report.valid && report.complete, "{report:?}");
    assert_eq!(report.surface.operations, 2);
    let parameter = report
        .findings
        .iter()
        .find(|f| f.rule_id == "parameters-documented" && f.target.starts_with("COPY"))
        .unwrap();
    assert_eq!(
        parameter.location.address.pointer,
        "/paths/~1pets~1{id}/parameters/0"
    );
    let missing = report
        .findings
        .iter()
        .find(|f| f.rule_id == "operation-id-present")
        .unwrap();
    assert_eq!(
        missing.location.address.pointer,
        "/paths/~1pets~1{id}/additionalOperations/COPY"
    );
    let fixed = fixes::apply(&serde_json::from_value(json!({"request":request,"keys":[missing.key],"expectedFiles":{report.manifest.files[0].uri.clone():report.manifest.files[0].sha256}})).unwrap()).unwrap();
    assert!(fixed.report.valid);
    assert!(fixed.revised.input.content.unwrap().contains("3.2.1"));
}

#[test]
fn reusable_media_types_and_streaming_item_schemas_are_typed_and_checked() {
    let mut value = source("3.2.1");
    value["components"] = json!({"schemas":{"Event":{"type":"object","properties":{"id":{"type":"integer","minimum":1}}}},"mediaTypes":{"Events":{"itemSchema":{"$ref":"#/components/schemas/Event"},"examples":{"stream":{"dataValue":[{"id":0,"$ref":"https://example.invalid/not-a-reference"}]}}}}});
    value["paths"]["/pets"]["get"]["responses"]["200"]["content"] =
        json!({"application/jsonl":{"$ref":"#/components/mediaTypes/Events"}});
    let report = audit(&request(value)).unwrap();
    assert!(report.valid && report.complete, "{:?}", report.diagnostics);
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.rule_id == "response-schema")
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.rule_id == "schema-example-valid"
                && f.location.address.pointer
                    == "/components/mediaTypes/Events/examples/stream/dataValue/0")
    );
}

#[test]
fn datavalue_and_serialized_json_examples_are_checked_without_fetching() {
    for example in [json!({"dataValue":4}), json!({"serializedValue":"4"})] {
        let mut value = source("3.2.1");
        value["paths"]["/pets"]["get"]["responses"]["200"]["content"]["application/json"]["examples"]
            ["pet"] = example;
        let report = audit(&request(value)).unwrap();
        assert!(report.valid && report.complete);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.rule_id == "schema-example-type")
        );
    }
    let mut value = source("3.2.1");
    value["paths"]["/pets"]["get"]["parameters"] = json!([{"name":"filter","in":"querystring","description":"Filter pets","content":{"application/json":{"schema":{"type":"object"},"examples":{"filter":{"dataValue":{"$ref":"https://example.invalid/data","$id":"literal"},"serializedValue":"{\"$ref\":\"https://example.invalid/data\",\"$id\":\"literal\"}"}}}}}]);
    let report = audit(&request(value)).unwrap();
    assert!(report.valid && report.complete, "{:?}", report.diagnostics);
}

#[test]
fn swagger_gets_converter_guidance_instead_of_a_readiness_score() {
    for source in [
        json!({"swagger":"2.0","info":{"title":"Old API","version":"1"},"paths":{}}),
        json!({"swaggerVersion":"1.2","apiVersion":"1","apis":[]}),
    ] {
        let report = audit(&request(source)).unwrap();
        assert!(!report.complete && !report.policy_passed);
        assert_eq!(report.score, 0.0);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.code.as_deref() == Some("SWAGGER_CONVERSION_REQUIRED")
                    && f.message.contains("Spec Converter"))
        );
        assert!(report.findings.iter().all(|f| !f.fixable));
    }
}
