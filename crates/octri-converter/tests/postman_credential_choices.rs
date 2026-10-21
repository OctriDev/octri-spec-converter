use octri_converter::{Config, ConversionReport, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SECRET: &str = "credential-secret-never-in-output";

fn request() -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://credentials/collection.json".into(),
            content: Some(json!({"$kind":"collection","name":"Credential choices","auth":[
                {"id":"public","name":"Public access","type":"noauth","rules":[{"key":"url","value":"https://api.example.test/public/*"}]},
                {"id":"private","name":"Private access","type":"bearer","credentials":[{"key":"token","value":SECRET}],"rules":[{"key":"url","value":"https://api.example.test/private/*"}]}
            ],"items":[{"$kind":"http-request","name":"List items","method":"GET","url":"https://api.example.test/items"}]}).to_string()),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    }
}

fn choice(input: &ConversionRequest, index: usize) -> Result<Value, Box<dyn std::error::Error>> {
    let content = input.input.content.as_deref().ok_or("content")?;
    Ok(
        json!({"sourceUri":input.input.uri,"sourceSha256":format!("{:x}",Sha256::digest(content.as_bytes())),
        "authPointer":"/auth","credentialIndex":index}),
    )
}

fn choose(
    input: &mut ConversionRequest,
    pointer: &str,
    selection: Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = serde_json::to_value(&input.config)?;
    config["credentialSelections"][pointer] = selection;
    input.config = serde_json::from_value(config)?;
    Ok(())
}

fn accept(input: &mut ConversionRequest, report: &ConversionReport) {
    input.config.accepted_decisions = report
        .decisions
        .iter()
        .map(|decision| decision.id.clone())
        .collect();
}

#[test]
fn conditional_auth_has_safe_choices_and_requires_review_after_selection()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request();
    let pending = convert(&input)?;
    assert_eq!(pending.status, Status::Blocked);
    let report = serde_json::to_value(&pending)?;
    let selection = &report["credentialSelectionRequests"][0];
    assert_eq!(selection["request"]["pointer"], "/items/0");
    assert_eq!(selection["authSource"]["pointer"], "/auth");
    assert!(selection["request"]["offset"].as_u64().is_some());
    assert_eq!(selection["choices"][1]["name"], "Private access");
    assert_eq!(selection["choices"][1]["authType"], "bearer");
    assert!(!selection.to_string().contains(SECRET));
    accept(&mut input, &pending);
    assert_eq!(convert(&input)?.status, Status::Blocked);

    let selected = choice(&input, 1)?;
    choose(&mut input, "/items/0", selected)?;
    let authored = convert(&input)?;
    assert!(authored.valid, "{:?}", authored.diagnostics);
    assert_eq!(authored.status, Status::NeedsReview);
    assert!(
        !authored
            .output
            .as_ref()
            .ok_or("output")?
            .content
            .contains(SECRET)
    );
    let decision = authored
        .decisions
        .iter()
        .find(|decision| decision.code == "POSTMAN_AUTH_SET_AUTHORED")
        .ok_or("authored decision")?;
    assert!(decision.review_required && !decision.reviewed);
    assert_eq!(decision.source.address.pointer, "/auth/1");
    accept(&mut input, &authored);
    let reviewed = convert(&input)?;
    assert_eq!(reviewed.status, Status::Complete);
    assert!(reviewed.handoff_allowed);
    Ok(())
}

#[test]
fn sibling_requests_can_choose_distinct_security_from_same_scope()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request();
    let mut source: Value = serde_json::from_str(input.input.content.as_deref().ok_or("content")?)?;
    source["items"].as_array_mut().ok_or("items")?.push(json!({"$kind":"http-request","name":"Private","method":"GET","url":"https://api.example.test/private"}));
    input.input.content = Some(source.to_string());
    let public = choice(&input, 0)?;
    let private = choice(&input, 1)?;
    choose(&mut input, "/items/0", public)?;
    choose(&mut input, "/items/1", private)?;
    let report = convert(&input)?;
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let document = report.document.ok_or("document")?;
    assert_eq!(document["paths"]["/items"]["get"]["security"], json!([]));
    assert_ne!(document["paths"]["/private"]["get"]["security"], json!([]));
    assert!(document["paths"]["/private"]["get"]["security"].is_array());
    Ok(())
}

#[test]
fn stale_or_invalid_bindings_never_emit_selected_credentials()
-> Result<(), Box<dyn std::error::Error>> {
    let original = request();
    let valid = choice(&original, 1)?;
    for (key, value) in [
        ("sourceUri", json!("memory://different/source.json")),
        ("sourceSha256", json!("0".repeat(64))),
        ("authPointer", json!("/items/0/auth")),
        ("credentialIndex", json!(99)),
    ] {
        let mut input = original.clone();
        let mut selection = valid.clone();
        selection[key] = value;
        choose(&mut input, "/items/0", selection)?;
        let report = convert(&input)?;
        assert_eq!(
            report.status,
            Status::Blocked,
            "{key}: {:?}",
            report.decisions
        );
        assert!(
            report
                .decisions
                .iter()
                .any(|decision| decision.code == "POSTMAN_AUTH_SELECTION_STALE")
        );
        assert!(
            !report
                .decisions
                .iter()
                .any(|decision| decision.code == "POSTMAN_AUTH_SET_AUTHORED")
        );
        accept(&mut input, &report);
        assert_eq!(convert(&input)?.status, Status::Blocked);
    }
    let mut input = original;
    choose(&mut input, "/items/0", valid)?;
    let reviewed = convert(&input)?;
    accept(&mut input, &reviewed);
    input.input.content.as_mut().ok_or("content")?.push(' ');
    let changed = convert(&input)?;
    assert_eq!(changed.status, Status::Blocked);
    assert!(
        changed
            .decisions
            .iter()
            .any(|decision| decision.code == "POSTMAN_AUTH_SELECTION_STALE")
    );
    Ok(())
}

#[test]
fn oversized_indices_remain_stale_without_platform_sized_deserialization()
-> Result<(), Box<dyn std::error::Error>> {
    for index in [u64::from(u32::MAX) + 1, u64::MAX] {
        let mut input = request();
        let mut selected = choice(&input, 1)?;
        selected["credentialIndex"] = json!(index);
        choose(&mut input, "/items/0", selected)?;
        let report = convert(&input)?;
        assert_eq!(report.status, Status::Blocked);
        assert!(
            report
                .decisions
                .iter()
                .any(|decision| decision.code == "POSTMAN_AUTH_SELECTION_STALE")
        );
        assert!(
            !report
                .decisions
                .iter()
                .any(|decision| decision.code == "POSTMAN_AUTH_SET_AUTHORED")
        );
        assert_eq!(report.credential_selection_requests[0].selected, None);
        assert!(!report.handoff_allowed);
    }
    Ok(())
}

#[test]
fn moved_requests_and_explicit_auth_overrides_cannot_reuse_old_choice()
-> Result<(), Box<dyn std::error::Error>> {
    for explicit_override in [false, true] {
        let mut input = request();
        let selected = choice(&input, 1)?;
        let mut source: Value =
            serde_json::from_str(input.input.content.as_deref().ok_or("content")?)?;
        if explicit_override {
            source["items"][0]["auth"] = json!({"type":"noauth"});
        }
        input.input.content = Some(source.to_string());
        let mut selected = selected;
        selected["sourceSha256"] = choice(&input, 1)?["sourceSha256"].clone();
        choose(
            &mut input,
            if explicit_override {
                "/items/0"
            } else {
                "/items/99"
            },
            selected,
        )?;
        let report = convert(&input)?;
        assert_eq!(report.status, Status::Blocked);
        assert!(
            report
                .decisions
                .iter()
                .any(|decision| decision.code == "POSTMAN_AUTH_SELECTION_UNUSED")
        );
    }
    Ok(())
}

#[test]
fn selected_unknown_fields_and_unsupported_auth_stay_blocked()
-> Result<(), Box<dyn std::error::Error>> {
    for unknown_field in [false, true] {
        let mut input = request();
        let mut source: Value =
            serde_json::from_str(input.input.content.as_deref().ok_or("content")?)?;
        if unknown_field {
            source["auth"][1]["disabled"] = json!(true);
        } else {
            source["auth"][1]["type"] = json!("jwt");
        }
        input.input.content = Some(source.to_string());
        let selected = choice(&input, 1)?;
        choose(&mut input, "/items/0", selected)?;
        let report = convert(&input)?;
        assert_eq!(report.status, Status::Blocked);
        accept(&mut input, &report);
        assert_eq!(convert(&input)?.status, Status::Blocked);
        if unknown_field {
            assert!(
                report
                    .decisions
                    .iter()
                    .any(|decision| decision.code == "POSTMAN_AUTH_FIELD_UNSUPPORTED")
            );
        }
    }
    Ok(())
}

#[test]
fn folder_scope_and_sets_without_ids_remain_request_specific()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request();
    let mut source: Value = serde_json::from_str(input.input.content.as_deref().ok_or("content")?)?;
    let mut auth = source["auth"].take();
    for set in auth.as_array_mut().ok_or("sets")? {
        set.as_object_mut().ok_or("set")?.remove("id");
    }
    let mut leaf = source["items"][0].clone();
    leaf["auth"] = json!({"type":"inherit"});
    source["auth"] = json!({"type":"noauth"});
    source["items"] =
        json!([{"$kind":"collection","name":"Private folder","auth":auth,"items":[leaf]}]);
    input.input.content = Some(source.to_string());
    let mut selected = choice(&input, 1)?;
    selected["authPointer"] = json!("/items/0/auth");
    choose(&mut input, "/items/0/items/0", selected)?;
    let report = convert(&input)?;
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    assert_eq!(
        report.credential_selection_requests[0]
            .auth_source
            .address
            .pointer,
        "/items/0/auth"
    );
    assert!(
        report.credential_selection_requests[0]
            .choices
            .iter()
            .all(|choice| choice.id.is_none())
    );
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "POSTMAN_AUTH_SET_AUTHORED"
                && decision.source.address.pointer == "/items/0/auth/1")
    );
    Ok(())
}

#[test]
fn authored_noauth_supports_every_target_without_weakening_review()
-> Result<(), Box<dyn std::error::Error>> {
    for target in octri_converter::TARGET_VERSIONS {
        let mut input = request();
        input.config.target_version = Some((*target).into());
        if *target == "2.0" {
            input.config.base_url = Some("https://api.example.test".into());
            input
                .config
                .drop_unsupported
                .insert("/paths/~1items/get/servers".into());
        }
        let selected = choice(&input, 0)?;
        choose(&mut input, "/items/0", selected)?;
        let report = convert(&input)?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_eq!(
            report.status,
            Status::NeedsReview,
            "{target}: {:?}",
            report.decisions
        );
        assert_eq!(
            report.document.as_ref().ok_or("document")?["paths"]["/items"]["get"]["security"],
            json!([])
        );
        accept(&mut input, &report);
        assert_eq!(convert(&input)?.status, Status::Complete);
    }
    Ok(())
}

#[test]
fn inherited_choice_metadata_shares_a_bounded_report_budget()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request();
    let mut source: Value = serde_json::from_str(input.input.content.as_deref().ok_or("content")?)?;
    source["items"]=json!((0..20).map(|index|json!({"$kind":"http-request","name":format!("Request {index}"),"method":"GET","url":format!("https://api.example.test/{index}")})).collect::<Vec<_>>());
    input.input.content = Some(source.to_string());
    input.config.limits.max_decisions = 8;
    let report = convert(&input)?;
    assert_eq!(report.status, Status::Blocked);
    assert!(!report.handoff_allowed);
    let units: usize = report
        .credential_selection_requests
        .iter()
        .map(|request| 1 + request.choices.len())
        .sum();
    assert!(units <= 8);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.id == "fidelity-coverage" && check.status == "failed")
    );
    Ok(())
}

#[test]
fn long_inherited_labels_cannot_multiply_report_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = request();
    let mut source: Value = serde_json::from_str(input.input.content.as_deref().ok_or("content")?)?;
    source["auth"][0]["name"] = json!("long credential label ".repeat(400));
    source["items"]=json!((0..10).map(|index|json!({"$kind":"http-request","name":format!("Request {index}"),"method":"GET","url":format!("https://api.example.test/{index}")})).collect::<Vec<_>>());
    input.input.content = Some(source.to_string());
    input.config.limits.max_output_bytes = 16 * 1024;
    let report = convert(&input)?;
    assert_eq!(report.status, Status::Blocked);
    let bytes = serde_json::to_vec(&report.credential_selection_requests)?.len();
    assert!(
        bytes <= 16 * 1024,
        "Unbounded repeated labels: {bytes} bytes"
    );
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.id == "fidelity-coverage" && check.status == "failed")
    );
    Ok(())
}

#[test]
fn credential_metadata_accepts_exact_byte_ceiling() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = request();
    let mut source: Value = serde_json::from_str(input.input.content.as_deref().ok_or("content")?)?;
    source["auth"][0]["name"] = json!("label ".repeat(300));
    source["items"] = json!((0..2).map(|index| json!({"$kind":"http-request","name":format!("Request {index}"),"method":"GET","url":format!("https://api.example.test/{index}")})).collect::<Vec<_>>());
    input.input.content = Some(source.to_string());
    let report = convert(&input)?;
    assert_eq!(report.credential_selection_requests.len(), 2);
    let exact_bytes = serde_json::to_vec(&report.credential_selection_requests)?.len();

    input.config.limits.max_output_bytes = exact_bytes;
    let exact = convert(&input)?;
    assert_eq!(exact.credential_selection_requests.len(), 2);
    assert_eq!(
        serde_json::to_vec(&exact.credential_selection_requests)?.len(),
        exact_bytes
    );
    assert!(
        !exact
            .checks
            .iter()
            .any(|check| check.id == "fidelity-coverage" && check.status == "failed")
    );

    input.config.limits.max_output_bytes = exact_bytes - 1;
    let over = convert(&input)?;
    assert_eq!(over.credential_selection_requests.len(), 1);
    assert!(
        over.checks
            .iter()
            .any(|check| check.id == "fidelity-coverage" && check.status == "failed")
    );
    assert!(!over.handoff_allowed);
    Ok(())
}

#[test]
fn large_inherited_arrays_stop_expanding_after_budget_exhaustion()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request();
    let mut source: Value = serde_json::from_str(input.input.content.as_deref().ok_or("content")?)?;
    source["auth"] = json!((0..128).map(|index| json!({"id":format!("set-{index}"),"type":"bearer","credentials":[{"key":"token","value":"s".repeat(4096)}],"rules":[{"key":"url","value":format!("https://api.example.test/{index}/*")}]})).collect::<Vec<_>>());
    source["items"] = json!((0..256).map(|index| json!({"$kind":"http-request","name":format!("Request {index}"),"method":"GET","url":format!("https://api.example.test/{index}")})).collect::<Vec<_>>());
    input.input.content = Some(source.to_string());
    input.config.limits.max_decisions = 16;
    let report = convert(&input)?;
    assert_eq!(report.status, Status::Blocked);
    assert!(report.credential_selection_requests.is_empty());
    assert!(report.decisions.len() <= 16);
    assert!(!report.handoff_allowed);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.id == "fidelity-coverage" && check.status == "failed")
    );
    Ok(())
}
