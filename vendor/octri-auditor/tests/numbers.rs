use octri_auditor::{AuditRequest, Input, audit};

#[test]
fn close_bounds_and_fractional_values_do_not_collapse_through_binary64()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"{"openapi":"3.1.2","info":{"title":"Exact bounds","version":"1"},"paths":{},"components":{"schemas":{"Empty":{"type":"number","minimum":0.100000000000000005,"maximum":0.1},"Fraction":{"type":"integer","enum":[9007199254740993.25]}}}}"#;
    let report = audit(&AuditRequest {
        input: Input {
            uri: "memory://exact/source.json".into(),
            content: Some(source.into()),
            generated: false,
        },
        files: Default::default(),
        config: Default::default(),
    })?;
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.rule_id == "schema-constraints")
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.rule_id == "schema-enum-domain")
    );
    Ok(())
}
