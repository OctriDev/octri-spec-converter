use octri_converter::{ConversionRequest, Status, convert};
use serde_json::{Value, json};

fn request(source: Value) -> Result<ConversionRequest, serde_json::Error> {
    serde_json::from_value(
        json!({"input":{"uri":"memory://review/source.json","content":source.to_string()}}),
    )
}

fn collection(items: Value) -> Value {
    json!({"info":{"name":"Review","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"auth":{"type":"noauth"},"item":items})
}

#[test]
fn selecting_any_duplicate_including_third_is_stable_and_missing_choices_block()
-> Result<(), Box<dyn std::error::Error>> {
    let source = collection(json!([
        {"name":"First","request":{"method":"GET","url":"https://api.example.com/pets"}},
        {"name":"Second","request":{"method":"GET","url":"https://api.example.com/pets"}},
        {"name":"Third","request":{"method":"GET","url":"https://api.example.com/pets"}}
    ]));
    let mut input = request(source)?;
    let initial = convert(&input)?;
    assert_eq!(initial.status, Status::Blocked);
    assert_eq!(initial.duplicate_requests[0].candidates.len(), 3);
    for (index, expected) in ["First", "Second", "Third"].into_iter().enumerate() {
        input
            .config
            .duplicate_choices
            .insert("GET /pets".into(), format!("/item/{index}"));
        let report = convert(&input)?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_eq!(report.status, Status::NeedsReview);
        assert!(!report.decisions.iter().any(|decision| decision.classification == octri_converter::Classification::Blocked));
        assert_eq!(
            report.document.ok_or("missing document")?["paths"]["/pets"]["get"]["summary"],
            expected
        );
    }
    input
        .config
        .duplicate_choices
        .insert("GET /pets".into(), "/item/99".into());
    let missing = convert(&input)?;
    assert_eq!(missing.status, Status::Blocked);
    assert!(
        missing
            .decisions
            .iter()
            .any(|decision| decision.code == "DUPLICATE_SELECTION_MISSING")
    );
    Ok(())
}

#[test]
fn conflicting_responses_keep_all_examples_media_and_case_insensitive_headers()
-> Result<(), Box<dyn std::error::Error>> {
    let source = collection(
        json!([{"name":"Pets","request":{"method":"GET","url":"https://api.example.com/pets"},"response":[
            {"name":"Object","code":200,"header":[{"key":"Content-Type","value":"application/json"},{"key":"X-Variant","value":"object"}],"body":"{\"id\":1}"},
            {"name":"Array","code":200,"header":[{"key":"Content-Type","value":"application/json"},{"key":"x-variant","value":"array"}],"body":"[\"cat\"]"},
            {"name":"Text","code":200,"header":[{"key":"Content-Type","value":"text/plain"}],"body":"cat"}
        ]}]),
    );
    let report = convert(&request(source)?)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview);
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "RESPONSE_VARIANTS" && decision.review_required)
    );
    let document = report.document.ok_or("missing document")?;
    let response = &document["paths"]["/pets"]["get"]["responses"]["200"];
    let media = &response["content"]["application/json"];
    assert_eq!(
        media["examples"]
            .as_object()
            .ok_or("missing examples")?
            .len(),
        2
    );
    assert_eq!(response["content"]["text/plain"]["example"], "cat");
    let headers = response["headers"].as_object().ok_or("missing headers")?;
    assert_eq!(headers.len(), 1);
    assert_eq!(
        headers["X-Variant"]["examples"]
            .as_object()
            .ok_or("missing header variants")?
            .len(),
        2
    );
    let schema = jsonschema::validator_for(&media["schema"])?;
    assert!(schema.is_valid(&json!({"id":1})));
    assert!(schema.is_valid(&json!(["cat"])));
    assert!(!schema.is_valid(&json!(false)));
    Ok(())
}

#[test]
fn swagger_defaults_to_openapi_and_encoded_transport_refs_resolve()
-> Result<(), Box<dyn std::error::Error>> {
    let source = json!({"swagger":"2.0","info":{"title":"Legacy","version":"1"},"parameters":{"named":{"name":"name","in":"query","type":"string"}},"paths":{"/pets":{"get":{"parameters":[{"$ref":"#/parameters/n%61med"}],"responses":{"204":{"description":"Empty"}}}}}});
    let mut input = request(source)?;
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.manifest.target_version, "3.1.2");
    assert_eq!(
        report.document.ok_or("missing output")?["paths"]["/pets"]["get"]["parameters"][0]["name"],
        "name"
    );
    input.config.target_version = Some("2.0".into());
    assert_eq!(convert(&input)?.manifest.target_version, "2.0");
    let modern = json!({"openapi":"3.2.1","info":{"title":"Modern","version":"1"},"paths":{}});
    assert_eq!(convert(&request(modern)?)?.manifest.target_version, "3.2.1");
    Ok(())
}
