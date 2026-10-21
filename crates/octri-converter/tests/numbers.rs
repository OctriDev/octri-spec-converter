use octri_converter::{
    ConversionReport, ConversionRequest, OutputFormat, Status, TARGET_VERSIONS, artifacts, convert,
};
use octri_parser::{Input, numbers};
use serde_json::json;

#[test]
fn every_target_keeps_exact_decimal_output_and_verified_report_roundtrips()
-> Result<(), Box<dyn std::error::Error>> {
    let content = r#"{"openapi":"3.1.2","info":{"title":"Exact decimal","version":"1"},"paths":{},"components":{"schemas":{"Exact":{"type":"number","example":0.100000000000000005},"Large":{"type":"integer","example":18446744073709551616}}}}"#;
    for target in TARGET_VERSIONS {
        for format in [OutputFormat::Json, OutputFormat::Yaml] {
            let request = ConversionRequest {
                input: Input {
                    uri: "memory://exact/source.json".into(),
                    content: Some(content.into()),
                    generated: false,
                },
                files: Default::default(),
                config: octri_converter::Config {
                    target_version: Some((*target).into()),
                    output_format: format,
                    ..Default::default()
                },
            };
            let report = convert(&request)?;
            assert!(report.valid, "{target}: {:?}", report.diagnostics);
            assert_eq!(
                report.status,
                Status::Complete,
                "{target}: {:?}",
                report.decisions
            );
            let output = report.output.as_ref().ok_or("missing output")?;
            let reparsed = octri_parser::parse_source(
                &output.uri,
                output.content.clone(),
                &Default::default(),
            )?;
            assert!(numbers::equivalent(
                report.document.as_ref().ok_or("missing document")?,
                &reparsed.document
            ));
            assert!(
                output.content.contains("0.100000000000000005"),
                "{target}: rounded decimal"
            );
            assert!(
                output.content.contains("18446744073709551616"),
                "{target}: rounded integer"
            );
            let raw = serde_json::to_string(&report)?;
            let decoded: ConversionReport = octri_parser::wire::decode_json("", &raw)?;
            artifacts::verify(&decoded)?;
            assert_eq!(artifacts::compare(&report, &decoded)?["changes"], json!([]));
            assert!(artifacts::export(&decoded, "html")?.is_string());
        }
    }
    Ok(())
}

#[test]
fn equivalent_config_numbers_keep_review_ids_and_output_hash_stable()
-> Result<(), Box<dyn std::error::Error>> {
    let mut request: ConversionRequest = serde_json::from_value(json!({
        "input":{"uri":"memory://exact/request.curl","content":"curl https://api.example.com/pets"},
        "config":{"overrides":{"/info/x-factor":1}}
    }))?;
    let first = convert(&request)?;
    request
        .config
        .overrides
        .insert("/info/x-factor".into(), serde_json::from_str("1.000e0")?);
    let second = convert(&request)?;
    assert_eq!(
        first.output.as_ref().map(|output| &output.sha256),
        second.output.as_ref().map(|output| &output.sha256)
    );
    assert_eq!(
        first
            .decisions
            .iter()
            .map(|decision| &decision.id)
            .collect::<Vec<_>>(),
        second
            .decisions
            .iter()
            .map(|decision| &decision.id)
            .collect::<Vec<_>>()
    );
    assert!(numbers::equivalent(
        first.document.as_ref().ok_or("missing document")?,
        second.document.as_ref().ok_or("missing document")?
    ));
    Ok(())
}

#[test]
fn transport_rejects_score_tampering_below_binary64_resolution()
-> Result<(), Box<dyn std::error::Error>> {
    let request: ConversionRequest = serde_json::from_value(json!({
        "input":{"uri":"memory://exact/source.json","content":"{\"openapi\":\"3.1.2\",\"info\":{\"title\":\"Scores\",\"version\":\"1\"},\"paths\":{}}"}
    }))?;
    let report = convert(&request)?;
    let mut encoded = serde_json::to_value(report)?;
    encoded["audit"]["score"] = serde_json::from_str("10.00000000000000001")?;
    let envelope =
        json!({"abiVersion":1,"action":"export","format":"json","report":encoded}).to_string();
    let response: serde_json::Value =
        serde_json::from_str(&octri_converter::wire::dispatch(&envelope))?;
    assert_eq!(response["Err"]["code"], "INVALID_REQUEST");
    Ok(())
}
