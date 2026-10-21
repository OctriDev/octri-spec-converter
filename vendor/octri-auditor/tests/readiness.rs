#![allow(clippy::unwrap_used)]
use octri_auditor::{AuditRequest, audit};
use serde_json::{Value, json};

fn document() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/clean.json")).unwrap()
}
fn request(value: Value) -> AuditRequest {
    serde_json::from_value(
        json!({"input":{"uri":"memory://audit/api.json","content":value.to_string()}}),
    )
    .unwrap()
}
fn has(value: Value, rule: &str) -> bool {
    audit(&request(value))
        .unwrap()
        .findings
        .iter()
        .any(|finding| finding.rule_id == rule)
}

#[test]
fn diagnostic_rules_have_failure_and_clear_boundaries() {
    let cases = [
        (
            "schema-constraint-applicability",
            json!({"type":"string","minimum":1}),
            json!({"type":"number","minimum":1}),
        ),
        (
            "schema-enum-domain",
            json!({"type":"string","enum":[3]}),
            json!({"type":"string","enum":["3"]}),
        ),
        (
            "schema-example-type",
            json!({"type":"integer","example":"1"}),
            json!({"type":"integer","example":1}),
        ),
        (
            "schema-example-valid",
            json!({"type":"string","minLength":3,"example":"x"}),
            json!({"type":"string","minLength":3,"example":"xxx"}),
        ),
        (
            "schema-constraints",
            json!({"type":"string","minLength":5,"maxLength":2}),
            json!({"type":"string","minLength":2,"maxLength":5}),
        ),
        (
            "schema-dialect",
            json!({"type":"string","nullable":true}),
            json!({"type":["string","null"]}),
        ),
        (
            "schema-composition",
            json!({"oneOf":[{"type":"string"},{"type":"number"}]}),
            json!({"type":"string"}),
        ),
        (
            "sdk-target-compatibility",
            json!({"type":"object","patternProperties":{"^x":{"type":"string"}}}),
            json!({"type":"object","properties":{"x":{"type":"string"}}}),
        ),
    ];
    for (rule, failing, passing) in cases {
        let mut value = document();
        value["components"]["schemas"]["Boundary"] = failing;
        assert!(has(value.clone(), rule), "failure fixture: {rule}");
        value["components"]["schemas"]["Boundary"] = passing;
        assert!(!has(value, rule), "clear fixture: {rule}");
    }
}

#[test]
fn namespace_children_and_flattened_types_cannot_hide_collisions() {
    let mut value = document();
    let operation = value["paths"]["/pets"]["get"].clone();
    value["paths"]["/pets"]["get"]["operationId"] = json!("getPets");
    value["paths"]["/child"]["get"] = operation.clone();
    value["paths"]["/child"]["get"]["operationId"] = json!("listChildren");
    value["paths"]["/child"]["get"]["x-octri-sdk-tags"] = json!("pets.get");
    let report = audit(&request(value.clone())).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.message.contains("Namespace child and method"))
    );
    value["paths"]["/pets"]["get"]["x-octri-sdk-tags"] = json!("a.bC");
    value["paths"]["/child"]["get"]["x-octri-sdk-tags"] = json!("aB.c");
    let report = audit(&request(value)).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.message.contains("flatten to the same"))
    );
}

#[test]
fn tool_names_use_protocol_boundaries_and_external_examples_are_unverified() {
    let mut value = document();
    value["paths"]["/pets"]["get"]["operationId"] = json!("pets.list");
    assert!(!has(value.clone(), "mcp-tool-name"));
    value["paths"]["/pets"]["get"]["operationId"] = json!("x".repeat(128));
    assert!(!has(value.clone(), "mcp-tool-name"));
    value["paths"]["/pets"]["get"]["operationId"] = json!("x".repeat(129));
    assert!(has(value.clone(), "mcp-tool-name"));
    value["paths"]["/pets"]["get"]["responses"]["200"]["content"]["application/json"]["examples"] =
        json!({"remote":{"externalValue":"https://example.invalid/payload"}});
    let report = audit(&request(value)).unwrap();
    assert!(!report.complete);
    assert_eq!(report.score, 0.0);
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.rule_id == "schema-example-unverified")
    );
}

#[test]
fn expiration_uses_caller_date_and_invalid_dates_are_rejected() {
    let mut value = document();
    value["paths"]["/pets"]["get"]
        .as_object_mut()
        .unwrap()
        .remove("summary");
    let mut request = request(value);
    request.config = serde_json::from_value(json!({"asOf":"2028-02-29","exclusions":[{"ruleId":"operation-documented","reason":"Migration owned by API team","expiresOn":"2028-02-29"}]})).unwrap();
    assert!(
        !audit(&request)
            .unwrap()
            .findings
            .iter()
            .any(|finding| finding.rule_id == "operation-documented")
    );
    request.config.as_of = Some("2028-03-01".into());
    assert!(
        audit(&request)
            .unwrap()
            .findings
            .iter()
            .any(|finding| finding.rule_id == "operation-documented")
    );
    request.config.as_of = Some("2027-02-29".into());
    assert_eq!(audit(&request).unwrap_err().code, "INVALID_CONFIG");
}

#[test]
fn each_declared_media_type_needs_its_own_contract() {
    let mut value = document();
    value["paths"]["/pets"]["get"]["responses"]["200"]["content"]["application/xml"] = json!({});
    let report = audit(&request(value)).unwrap();
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.rule_id == "response-schema")
        .unwrap();
    assert_eq!(
        finding.fix.as_ref().unwrap().media_type.as_deref(),
        Some("application/xml")
    );
}

#[test]
fn authored_schema_text_preserves_large_integers_and_rejects_duplicate_keys() {
    let mut value = document();
    value["paths"]["/pets"]["get"]["responses"]["200"]["content"] = json!({"application/json":{}});
    let request = request(value);
    let report = audit(&request).unwrap();
    let key = report
        .findings
        .iter()
        .find(|finding| finding.rule_id == "response-schema")
        .unwrap()
        .key
        .clone();
    let mut fix = octri_auditor::fixes::FixRequest {
        expected_files: octri_auditor::fixes::hashes(&report),
        request,
        keys: vec![key.clone()],
        inputs: [(
            key.clone(),
            json!({"schemaJson":"{\"type\":\"integer\",\"maximum\":18446744073709551615}"}),
        )]
        .into_iter()
        .collect(),
    };
    let revised = octri_auditor::fixes::apply(&fix).unwrap();
    assert!(
        revised
            .revised
            .input
            .content
            .unwrap()
            .contains("18446744073709551615")
    );
    fix.inputs.insert(
        key,
        json!({"schemaJson":"{\"type\":\"integer\",\"type\":\"string\"}"}),
    );
    assert_eq!(
        octri_auditor::fixes::apply(&fix).unwrap_err().code,
        "INVALID_AUTHORED_SCHEMA"
    );
}
