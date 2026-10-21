use octri_auditor::{AuditRequest, audit, audit_with_analysis};
use serde_json::json;

#[test]
fn shared_analysis_is_strict_and_audit_results_remain_identical()
-> Result<(), Box<dyn std::error::Error>> {
    for content in [
        json!({"openapi":"3.1.2","info":{"title":"Shared","version":"1"},"paths":{}}).to_string(),
        json!({"openapi":"3.1.2","info":{"title":"Shared","version":"1"},"paths":{"/pets":{"get":{"responses":{"200":{"description":"Pets"}},"parameters":[{"name":"id","in":"path","required":false,"schema":{"type":"string"}}]}}}}).to_string(),
        "{ invalid JSON".into(),
    ] {
        let request: AuditRequest = serde_json::from_value(json!({"input":{"uri":"memory://audit/api.json","content":content}}))?;
        let combined = audit_with_analysis(&request)?;
        assert_eq!(serde_json::to_value(&combined.audit)?, serde_json::to_value(audit(&request)?)?);
        assert_eq!(combined.audit.valid, combined.analysis.valid);
        if !combined.analysis.valid { assert!(!combined.audit.complete); assert_eq!(combined.audit.score, 0.0); }
    }
    Ok(())
}
