use octri_converter::{ConversionRequest, Status, artifacts, convert};
use serde_json::json;

#[test]
fn artifact_preserves_original_root_bytes_and_hash() -> Result<(), Box<dyn std::error::Error>> {
    let content =
        "{\"openapi\":\"3.1.2\",\"info\":{\"title\":\"Root\",\"version\":\"1\"},\"paths\":{}}\n";
    let request: ConversionRequest = serde_json::from_value(json!({
        "input":{"uri":"memory://project/root.json","content":content}
    }))?;
    let report = convert(&request)?;
    let artifact = artifacts::export(&report, "artifact")?;
    let original = artifact["files"]
        .as_array()
        .ok_or("missing files")?
        .iter()
        .find(|entry| entry["sourceUri"] == "memory://project/root.json")
        .ok_or("missing original root")?;
    assert_eq!(original["content"], content);
    assert_eq!(original["sha256"], report.manifest.files[0].sha256);
    Ok(())
}

#[test]
fn embedded_readiness_preserves_semantic_operation_verbs() -> Result<(), Box<dyn std::error::Error>>
{
    let mut document: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/clean.json"))?;
    for (index, id) in [
        "findPetsByStatus",
        "loginUser",
        "logoutUser",
        "placeOrder",
        "ProvisionGetToken",
        "ReconcileReadReceipt",
        "OnboardUserGet",
    ]
    .iter()
    .enumerate()
    {
        let mut operation = document["paths"]["/pets"]["get"].clone();
        operation["operationId"] = json!(id);
        document["paths"][format!("/naming/{index}")]["get"] = operation;
    }
    let request: ConversionRequest = serde_json::from_value(json!({
        "input": {"uri": "memory://project/naming.json", "content": document.to_string()},
        "config": {"targetVersion": "3.2.1"}
    }))?;
    let report = convert(&request)?;
    assert!(report.valid);
    let audit = report.audit.ok_or("missing readiness audit")?;
    assert_eq!(audit.manifest.ruleset_version, "1.0.3");
    assert!(
        !audit
            .findings
            .iter()
            .any(|finding| finding.rule_id == "operation-id-naming")
    );
    Ok(())
}

#[test]
fn mapping_manifest_keeps_decision_addresses_and_retained_inventory()
-> Result<(), Box<dyn std::error::Error>> {
    let request: ConversionRequest = serde_json::from_value(json!({
        "input":{"uri":"memory://project/request.curl","content":"curl https://api.example.com/pets"}
    }))?;
    let report = convert(&request)?;
    let manifest = artifacts::export(&report, "manifest")?;
    assert_eq!(manifest["root"], report.manifest.root);
    assert_eq!(
        manifest["outputSha256"],
        report.output.as_ref().ok_or("missing output")?.sha256
    );
    let mappings = manifest["mappings"].as_array().ok_or("missing mappings")?;
    assert_eq!(mappings.len(), report.decisions.len());
    for (mapping, decision) in mappings.iter().zip(&report.decisions) {
        assert_eq!(mapping["source"]["uri"], decision.source.address.uri);
        assert_eq!(mapping["outputPointers"], json!(decision.output_pointers));
    }
    assert_eq!(
        report
            .inspection
            .as_ref()
            .ok_or("missing inventory")?
            .statistics
            .operations,
        1
    );
    Ok(())
}

#[test]
fn incomplete_static_readiness_requires_explicit_review_without_fetching()
-> Result<(), Box<dyn std::error::Error>> {
    let source = json!({"openapi":"3.1.2","info":{"title":"Examples","version":"1"},"paths":{
        "/pets":{"get":{"responses":{"200":{"description":"Pets","content":{"application/json":{
            "schema":{"type":"object"},"examples":{"remote":{"externalValue":"https://example.invalid/payload"}}
        }}}}}}
    }});
    let mut request: ConversionRequest = serde_json::from_value(json!({
        "input":{"uri":"memory://project/root.json","content":source.to_string()}
    }))?;
    let report = convert(&request)?;
    assert!(report.valid);
    assert_eq!(report.status, Status::NeedsReview);
    assert!(!report.handoff_allowed);
    assert!(!report.audit.as_ref().ok_or("missing audit")?.complete);
    let decision = report
        .decisions
        .iter()
        .find(|decision| decision.code == "READINESS_AUDIT_INCOMPLETE")
        .ok_or("missing coverage decision")?;
    request
        .config
        .accepted_decisions
        .insert(decision.id.clone());
    let reviewed = convert(&request)?;
    assert_eq!(reviewed.status, Status::Complete);
    assert!(
        !reviewed
            .audit
            .as_ref()
            .ok_or("missing reviewed audit")?
            .complete
    );
    Ok(())
}
