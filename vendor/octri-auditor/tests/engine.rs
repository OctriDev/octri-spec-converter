#![allow(clippy::unwrap_used)]
use octri_auditor::{AuditRequest, Config, Input, Severity, audit, baseline, export, fixes, wire};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn good() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/clean.json")).unwrap()
}
fn request(value: Value) -> AuditRequest {
    AuditRequest {
        input: Input {
            uri: "memory://audit/openapi.json".into(),
            content: Some(value.to_string()),
            generated: false,
        },
        files: BTreeMap::new(),
        config: Config::default(),
    }
}
fn rules(value: Value) -> Vec<String> {
    audit(&request(value))
        .unwrap()
        .findings
        .into_iter()
        .map(|f| f.rule_id)
        .collect()
}

#[test]
fn clean_document_is_complete_and_deterministic() {
    let request = request(good());
    let report = audit(&request).unwrap();
    assert!(
        report.valid && report.complete && report.policy_passed,
        "{:?}",
        report.findings
    );
    assert_eq!(report.score, 10.0);
    assert_eq!(report.views.len(), 4);
    assert_eq!(report.manifest.profiles.len(), 12);
    assert_eq!(
        serde_json::to_string(&report).unwrap(),
        serde_json::to_string(&audit(&request).unwrap()).unwrap()
    );
    assert_eq!(
        report
            .by_rule
            .iter()
            .filter(|rule| rule.weight > 0.0)
            .count(),
        14
    );
}

#[test]
fn scored_rules_have_positive_and_negative_fixtures() {
    type Mutation = (&'static str, Box<dyn Fn(&mut Value)>);
    let mutations: Vec<Mutation> = vec![
        (
            "operation-id-present",
            Box::new(|v| {
                v["paths"]["/pets"]["get"]
                    .as_object_mut()
                    .unwrap()
                    .remove("operationId");
            }),
        ),
        (
            "operation-id-naming",
            Box::new(|v| v["paths"]["/pets"]["get"]["operationId"] = json!("PetsPetsGet")),
        ),
        (
            "operation-documented",
            Box::new(|v| {
                v["paths"]["/pets"]["get"]
                    .as_object_mut()
                    .unwrap()
                    .remove("summary");
            }),
        ),
        (
            "operation-tagged",
            Box::new(|v| v["paths"]["/pets"]["get"]["tags"] = json!([])),
        ),
        (
            "error-responses",
            Box::new(|v| {
                v["paths"]["/pets"]["get"]["responses"]
                    .as_object_mut()
                    .unwrap()
                    .remove("400");
            }),
        ),
        (
            "response-schema",
            Box::new(|v| {
                v["paths"]["/pets"]["get"]["responses"]["200"]["content"]["application/json"]
                    .as_object_mut()
                    .unwrap()
                    .remove("schema");
            }),
        ),
        (
            "server-url",
            Box::new(|v| {
                v.as_object_mut().unwrap().remove("servers");
            }),
        ),
        (
            "security-declared",
            Box::new(|v| {
                v["components"]
                    .as_object_mut()
                    .unwrap()
                    .remove("securitySchemes");
            }),
        ),
        (
            "parameters-documented",
            Box::new(|v| {
                v["paths"]["/pets"]["get"]["parameters"] =
                    json!([{"name":"limit","in":"query","schema":{"type":"integer"}}])
            }),
        ),
        (
            "request-body-documented",
            Box::new(|v| {
                v["paths"]["/pets"]["get"]["requestBody"] =
                    json!({"content":{"application/json":{"schema":{"type":"object"}}}})
            }),
        ),
        (
            "path-parameters-declared",
            Box::new(|v| {
                let operation = v["paths"]["/pets"].take();
                v["paths"].as_object_mut().unwrap().remove("/pets");
                v["paths"]["/pets/{id}"] = operation;
            }),
        ),
        (
            "duplicate-paths",
            Box::new(|v| {
                let mut item = v["paths"]["/pets"].clone();
                item["get"]["operationId"] = json!("getPetByName");
                item["parameters"] = json!([{"name":"name","in":"path","required":true,"description":"Pet name","schema":{"type":"string"}}]);
                v["paths"]["/pets/{name}"] = item.clone();
                item["get"]["operationId"] = json!("getPetById");
                item["parameters"][0]["name"] = json!("id");
                v["paths"]["/pets/{id}"] = item;
            }),
        ),
        (
            "operation-id-unique",
            Box::new(|v| {
                v["paths"]["/other"] = v["paths"]["/pets"].clone();
            }),
        ),
        (
            "shared-schemas-referenced",
            Box::new(|v| {
                v["paths"]["/pets"]["get"]["responses"]["200"]["content"]["application/json"]["schema"] = json!({"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"},"c":{"type":"string"}}});
                v["paths"]["/other"] = v["paths"]["/pets"].clone();
                v["paths"]["/other"]["get"]["operationId"] = json!("otherPets");
            }),
        ),
    ];
    for (id, mutate) in mutations {
        assert!(!rules(good()).contains(&id.into()), "positive {id}");
        let mut value = good();
        mutate(&mut value);
        assert!(rules(value).contains(&id.into()), "negative {id}");
    }
}

#[test]
fn all_success_responses_checked_and_bodyless_boundaries_exempt() {
    for status in ["204", "205"] {
        let mut value = good();
        value["paths"]["/pets"]["get"]["responses"] = json!({status:{"description":"No body"}});
        assert!(!rules(value).contains(&"response-schema".into()));
    }
    let mut value = good();
    value["paths"]["/pets"]["get"]["responses"]["201"] = json!({"description":"Created"});
    assert!(rules(value).contains(&"response-schema".into()));
    let mut value = good();
    value["paths"]["/pets"]["head"] = value["paths"]["/pets"]["get"].take();
    value["paths"]["/pets"]
        .as_object_mut()
        .unwrap()
        .remove("get");
    value["paths"]["/pets"]["head"]["responses"]["200"] = json!({"description":"Headers only"});
    assert!(!rules(value).contains(&"response-schema".into()));
}

#[test]
fn integrity_cannot_be_ignored_or_disabled() {
    let mut req = request(json!({"openapi":"3.1.2","info":{"title":"Bad"},"paths":{}}));
    let report = audit(&req).unwrap();
    assert!(!report.valid && !report.policy_passed);
    assert_eq!(report.score, 0.0);
    req.config.ignored = report.findings.iter().map(|f| f.key.clone()).collect();
    assert!(!audit(&req).unwrap().findings.is_empty());
    req.config.rules.insert(
        "spec-integrity".into(),
        octri_auditor::RuleSetting {
            enabled: Some(false),
            severity: None,
        },
    );
    assert_eq!(audit(&req).unwrap_err().code, "INVALID_CONFIG");
}

#[test]
fn malformed_duplicate_keys_missing_refs_and_versions_block() {
    for content in [
        "{",
        "openapi: 3.1.2\nopenapi: 3.0.3\n",
        "{\"openapi\":\"3.1.2\",\"openapi\":\"3.0.3\"}",
    ] {
        let mut req = request(good());
        req.input.content = Some(content.into());
        let report = audit(&req).unwrap();
        assert!(
            !report.valid && !report.policy_passed && report.score == 0.0,
            "{content}"
        );
    }
    for version in ["2.0", "9.0.0"] {
        let mut value = good();
        value["openapi"] = json!(version);
        assert!(!audit(&request(value)).unwrap().complete);
    }
    let mut value = good();
    value["components"]["schemas"]["Pet"] = json!({"$ref":"http://127.0.0.1/secrets"});
    let report = audit(&request(value)).unwrap();
    assert!(!report.valid);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.contains("RESOLVE") || d.code.contains("IO") || d.code.contains("REF")),
        "{:?}",
        report.diagnostics
    );
}

#[test]
fn virtual_reference_provenance_and_inherited_parameters() {
    let mut value = good();
    value["paths"]["/pets"]["parameters"] = json!([{"$ref":"parameter.yaml"}]);
    let mut req = request(value);
    req.files.insert(
        "parameter.yaml".into(),
        "name: limit\nin: query\nschema:\n  type: integer\n".into(),
    );
    let report = audit(&req).unwrap();
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.rule_id == "parameters-documented")
    );
    assert_eq!(report.manifest.files.len(), 2);
    let mut value = good();
    value["paths"]["/pets"]["parameters"] = json!([{"name":"limit","in":"query","description":"Inherited","schema":{"type":"integer"}}]);
    value["paths"]["/pets"]["get"]["parameters"] =
        json!([{"name":"limit","in":"query","schema":{"type":"integer"}}]);
    assert!(rules(value).contains(&"parameters-documented".into()));
}

#[test]
fn source_locations_escape_pointers_and_avoid_example_payloads() {
    let mut value = good();
    value["components"]["schemas"]["a/b~c"] = json!({"type":"string","example":12});
    value["x-data"] = json!({"schema":{"type":"string","example":99}});
    let report = audit(&request(value)).unwrap();
    let found: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.rule_id == "schema-example-type")
        .collect();
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].location.address.pointer,
        "/components/schemas/a~1b~0c/example"
    );
    assert!(found[0].location.line.is_some());
}

#[test]
fn schema_rules_are_dialect_aware_and_constraints_validate_examples() {
    let mut value = good();
    value["components"]["schemas"]["Pet"] = json!({"type":"string","minLength":3,"maxLength":1,"enum":[1,"a"],"example":"x","nullable":true,"oneOf":[{"type":"string"},{"type":"number"}],"patternProperties":{"^x":{"type":"string"}}});
    let ids = rules(value);
    for rule in [
        "schema-enum-domain",
        "schema-constraints",
        "schema-example-valid",
        "schema-dialect",
        "schema-composition",
        "sdk-target-compatibility",
    ] {
        assert!(ids.contains(&rule.into()), "{rule}: {ids:?}");
    }
    let mut value = good();
    value["components"]["schemas"]["Pet"] = json!({"type":"integer","minLength":1});
    assert!(rules(value).contains(&"schema-constraint-applicability".into()));
    let mut value = good();
    value["openapi"] = json!("3.0.3");
    value["components"]["schemas"]["Pet"] = json!({"type":"string","nullable":true,"example":null});
    assert!(!rules(value).contains(&"schema-example-type".into()));
}

#[test]
fn exclusions_baselines_and_diff_keep_evidence() {
    let mut value = good();
    value["paths"]["/pets"]["get"]["tags"] = json!([]);
    let mut req = request(value);
    req.config.fail_on = Severity::Medium;
    let first = audit(&req).unwrap();
    assert!(!first.policy_passed);
    req.config.baseline = Some(baseline(&first));
    req.config.fail_on_new = true;
    assert!(audit(&req).unwrap().policy_passed);
    req.config.fail_on_new = false;
    req.config.ignored = first.findings.iter().map(|f| f.key.clone()).collect();
    let ignored = audit(&req).unwrap();
    assert_eq!(ignored.ignored_findings.len(), first.findings.len());
    assert_eq!(ignored.score, 10.0);
    let clean = audit(&request(good())).unwrap();
    let diff = export::compare(&first, &clean).unwrap();
    assert!(!diff.resolved.is_empty());
    let mut wrong = clean.clone();
    wrong.manifest.ruleset_version = "next".into();
    assert!(export::compare(&first, &wrong).is_err());
}

#[test]
fn reviewed_patches_validate_hashes_reaudit_and_preserve_undo() {
    let mut value = good();
    value["paths"]["/pets"]["get"]
        .as_object_mut()
        .unwrap()
        .remove("operationId");
    let req = request(value);
    let report = audit(&req).unwrap();
    let key = report
        .findings
        .iter()
        .find(|f| f.rule_id == "operation-id-present")
        .unwrap()
        .key
        .clone();
    let mut fix = fixes::FixRequest {
        request: req.clone(),
        keys: vec![key],
        expected_files: fixes::hashes(&report),
        inputs: BTreeMap::new(),
    };
    let result = fixes::apply(&fix).unwrap();
    assert_eq!(result.original.input.content, req.input.content);
    assert!(
        !result
            .report
            .findings
            .iter()
            .any(|f| f.rule_id == "operation-id-present")
    );
    fix.expected_files.clear();
    assert_eq!(fixes::apply(&fix).unwrap_err().code, "STALE_PREVIEW");
}

#[test]
fn reports_escape_hostile_html_and_actions_reject_bad_transport() {
    let mut req = request(good());
    req.input.uri = "memory://audit/<script>alert(1)</script>.json".into();
    let report = audit(&req).unwrap();
    assert!(!export::html(&report).contains("<script>"));
    assert_eq!(export::sarif(&report)["version"], "2.1.0");
    for input in [
        "{}",
        "{\"abiVersion\":2,\"action\":\"rules\"}",
        "{\"abiVersion\":1,\"action\":\"rules\",\"extra\":true}",
    ] {
        assert!(wire::dispatch(input).contains("Err"));
    }
}

#[test]
fn finding_limit_blocks_green_outcomes() {
    let mut req = request(good());
    req.input.content = Some(
        json!({"openapi":"3.1.2","info":{"title":"Bad"},"paths":{"/x":{"get":{"responses":{}}}}})
            .to_string(),
    );
    req.config.max_findings = 1;
    let report = audit(&req).unwrap();
    assert!(!report.complete && !report.policy_passed);
    assert_eq!(report.score, 0.0);
}

#[test]
fn examples_resolve_local_external_and_recursive_schemas_offline() {
    for reference in ["#/components/schemas/Pet", "pet.json"] {
        let mut value = good();
        value["components"]["schemas"]["Pet"] = json!({"type":"object","required":["name"],"properties":{"name":{"type":"string","minLength":2},"friend":{"$ref":"#/components/schemas/Pet"}}});
        value["paths"]["/pets"]["get"]["responses"]["200"]["content"]["application/json"] =
            json!({"schema":{"$ref":reference},"example":{"name":"x"}});
        let mut req = request(value);
        if reference == "pet.json" {
            req.files.insert("pet.json".into(),json!({"type":"object","required":["name"],"properties":{"name":{"type":"string","minLength":2}}}).to_string());
        }
        let report = audit(&req).unwrap();
        assert!(report.valid, "{:?}", report.diagnostics);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.rule_id == "schema-example-valid"),
            "{reference}: {:?}",
            report.findings
        );
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.rule_id == "schema-example-unverified"),
            "{reference}: {:?}",
            report.findings
        );
    }
}

#[test]
fn generated_names_fold_acronyms_and_namespace_collisions() {
    for (first, second) in [
        ("get-user", "get_user"),
        ("getOauth2", "getOAuth2"),
        ("getPets", "getPet"),
    ] {
        let mut value = good();
        value["paths"]["/pets"]["get"]["operationId"] = json!(first);
        value["paths"]["/other"] = value["paths"]["/pets"].clone();
        value["paths"]["/other"]["get"]["operationId"] = json!(second);
        assert!(
            rules(value).contains(&"operation-id-unique".into()),
            "{first} / {second}"
        );
    }
}
