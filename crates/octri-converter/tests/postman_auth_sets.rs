//! Collection v3 credential selection must never fall through uncertain rules.
use octri_converter::{ConversionRequest, Status, convert};
use serde_json::{Value, json};

fn request(auth: Value) -> Result<ConversionRequest, serde_json::Error> {
    serde_json::from_value(json!({"input":{
        "uri":"memory://fixtures/credentials.json",
        "content":json!({"$kind":"collection","name":"Credential sets","auth":auth,
            "items":[{"$kind":"http-request","name":"Protected","method":"GET",
                "url":"https://api.example.test/items"}]}).to_string()
    },"config":{"targetVersion":"3.2.1"}}))
}

#[test]
fn unconditional_first_set_preserves_security_and_physical_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    for rules in [None, Some(json!([]))] {
        let mut first = json!({"id":"protected","name":"Bearer","type":"bearer",
            "credentials":[{"key":"token","value":"retained-only-secret"}]});
        if let Some(rules) = rules {
            first["rules"] = rules;
        }
        let input = request(json!([first,{"id":"public","name":"Later public set",
            "type":"noauth","rules":[{"key":"url","value":"*"}]}]))?;
        let report = convert(&input)?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
        assert_eq!(report.original.input.content, input.input.content);
        let selection = report
            .decisions
            .iter()
            .find(|decision| decision.code == "POSTMAN_AUTH_SET_SELECTED")
            .ok_or("missing selection evidence")?;
        assert_eq!(selection.source.address.pointer, "/auth/0");
        assert!(selection.source.offset.is_some());
        let document = report.document.ok_or("missing output")?;
        let requirements = document["paths"]["/items"]["get"]["security"][0]
            .as_object()
            .ok_or("missing security")?;
        let name = requirements.keys().next().ok_or("anonymous output")?;
        assert_eq!(
            document["components"]["securitySchemes"][name]["scheme"],
            "bearer"
        );
        assert!(
            !report
                .output
                .ok_or("missing artifact")?
                .content
                .contains("retained-only-secret")
        );
    }
    Ok(())
}

#[test]
fn credential_arrays_keep_root_folder_and_request_inheritance()
-> Result<(), Box<dyn std::error::Error>> {
    let auth = |name: &str, kind: &str| {
        let mut credential = json!({"id":name,"name":name,"type":kind,"rules":[]});
        if kind != "noauth" {
            credential["credentials"] = json!([]);
        }
        json!([credential])
    };
    let mut input = request(auth("root", "bearer"))?;
    let mut source: Value = serde_json::from_str(input.input.content.as_ref().ok_or("source")?)?;
    source["items"] = json!([
        {"$kind":"http-request","name":"Root","method":"GET","url":"https://api.example.test/root"},
        {"$kind":"collection","name":"Folder","auth":auth("folder","noauth"),"items":[
            {"$kind":"http-request","name":"Folder","method":"GET","url":"https://api.example.test/folder","auth":{"type":"inherit"}}]},
        {"$kind":"http-request","name":"Request","method":"GET","url":"https://api.example.test/request","auth":auth("request","basic")}
    ]);
    input.input.content = Some(source.to_string());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let document = report.document.ok_or("output")?;
    assert_eq!(document["paths"]["/folder"]["get"]["security"], json!([]));
    for (name, scheme) in [("root", "bearer"), ("request", "basic")] {
        let security = document["paths"][format!("/{name}")]["get"]["security"][0]
            .as_object()
            .ok_or("requirement")?;
        let key = security.keys().next().ok_or("anonymous")?;
        assert_eq!(
            document["components"]["securitySchemes"][key]["scheme"],
            scheme
        );
    }
    for pointer in ["/auth/0", "/items/1/auth/0", "/items/2/auth/0"] {
        assert!(
            report
                .decisions
                .iter()
                .any(|decision| decision.code == "POSTMAN_AUTH_SET_SELECTED"
                    && decision.source.address.pointer == pointer)
        );
    }
    Ok(())
}

#[test]
fn explicit_noauth_first_set_stays_anonymous_for_every_target()
-> Result<(), Box<dyn std::error::Error>> {
    for target in [
        "2.0", "3.0.0", "3.0.1", "3.0.2", "3.0.3", "3.0.4", "3.1.0", "3.1.1", "3.1.2", "3.2.0",
        "3.2.1",
    ] {
        let mut input = request(json!([
            {"id":"public","name":"Public","type":"noauth","rules":[]},
            {"id":"unused","name":"Unreachable","type":"basic","credentials":[]}
        ]))?;
        input.config.target_version = Some(target.into());
        if target == "2.0" {
            input.config.base_url = Some("https://api.example.test".into());
            input
                .config
                .drop_unsupported
                .insert("/paths/~1items/get/servers".into());
        }
        let report = convert(&input)?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_ne!(
            report.status,
            Status::Blocked,
            "{target}: {:?}",
            report.decisions
        );
        let document = report.document.ok_or("missing output")?;
        assert_eq!(document["paths"]["/items"]["get"]["security"], json!([]));
        if target == "2.0" {
            assert_eq!(document["host"], "api.example.test");
            assert_eq!(document["schemes"], json!(["https"]));
        }
    }
    Ok(())
}

#[test]
fn unknown_earlier_rules_never_fall_through_to_anonymous_fallback()
-> Result<(), Box<dyn std::error::Error>> {
    for rules in [
        json!([{"key":"url","value":"https://other.example.test/*"}]),
        json!(null),
        json!({}),
    ] {
        let mut input = request(json!([
            {"id":"protected","name":"Conditional","type":"basic","credentials":[],"rules":rules},
            {"id":"fallback","name":"Public fallback","type":"noauth"}
        ]))?;
        let report = convert(&input)?;
        assert_eq!(report.status, Status::Blocked);
        assert!(
            report
                .decisions
                .iter()
                .any(|decision| decision.code == "POSTMAN_AUTH_RULES")
        );
        input.config.accepted_decisions = report
            .decisions
            .iter()
            .map(|decision| decision.id.clone())
            .collect();
        assert_eq!(convert(&input)?.status, Status::Blocked);
    }
    Ok(())
}

#[test]
fn unknown_first_set_fields_cannot_select_anonymous_access()
-> Result<(), Box<dyn std::error::Error>> {
    for key in ["disabled", "x-selector"] {
        let mut first = json!({"id":"public","name":"Potentially disabled","type":"noauth"});
        first[key] = json!(true);
        let mut input = request(json!([
            first,
            {"id":"protected","name":"Protected fallback","type":"basic","credentials":[]}
        ]))?;
        let report = convert(&input)?;
        assert_eq!(report.status, Status::Blocked);
        let decision = report
            .decisions
            .iter()
            .find(|decision| decision.code == "POSTMAN_AUTH_FIELD_UNSUPPORTED")
            .ok_or("missing unknown-field blocker")?;
        assert_eq!(decision.source.address.pointer, format!("/auth/0/{key}"));
        assert!(
            !report
                .decisions
                .iter()
                .any(|decision| decision.code == "POSTMAN_AUTH_SET_SELECTED")
        );
        input.config.accepted_decisions = report
            .decisions
            .iter()
            .map(|decision| decision.id.clone())
            .collect();
        assert_eq!(convert(&input)?.status, Status::Blocked);
    }
    Ok(())
}

#[test]
fn empty_invalid_or_unsupported_sets_never_gain_valid_authentication()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(convert(&request(json!([]))?)?.status, Status::Blocked);
    for auth in [json!([null]), json!([{"id":"bad","name":"Invalid"}])] {
        assert!(convert(&request(auth)?).is_err());
    }
    let report = convert(&request(json!([
        {"id":"jwt","name":"Unsupported","type":"jwt","credentials":[]},
        {"id":"public","name":"Fallback","type":"noauth"}
    ]))?)?;
    assert_eq!(report.status, Status::Blocked);
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "AUTH_UNREPRESENTABLE")
    );
    Ok(())
}
