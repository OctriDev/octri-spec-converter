use octri_converter::{Config, ConversionRequest, InputFormat, Status, convert, detect};
use octri_parser::Input;
use serde_json::{Value, json};

fn request(source: Value) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://fixtures/collection.json".into(),
            content: Some(source.to_string()),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    }
}

fn collection(items: Value) -> Value {
    json!({"$kind":"collection","name":"Postman 3","auth":{"type":"noauth"},"items":items})
}

#[test]
fn detects_v3_and_preserves_http_observations_and_physical_locations()
-> Result<(), Box<dyn std::error::Error>> {
    let mut source = collection(json!([{
        "$kind":"collection","name":"Pets","items":[{
            "$kind":"http-request","name":"Create pet","method":"POST",
            "url":"{{base}}/pets/:id?stale=1",
            "queryParams":[{"key":"disabled","value":"1","disabled":true},{"key":"tag","value":"a"},{"key":"tag","value":"b"}],
            "pathVariables":[{"key":"id","value":"123"}],
            "body":{"type":"json","content":"{\"id\":18446744073709551615}"},
            "examples":[{"$kind":"http-example","name":"Created","response":{
                "statusCode":201,"statusText":"Created","body":{"type":"json","content":"{\"id\":18446744073709551615}"}
            }}],
            "scripts":[{"type":"beforeRequest","code":"throw new Error('never execute')"}]
        }]
    }]));
    source["variables"] = json!([{"key":"base","value":"https://wrong.example"}]);
    let mut input = request(source);
    input
        .config
        .variables
        .insert("base".into(), "https://api.example.com".into());
    assert_eq!(detect(&input)?, InputFormat::Postman);
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview);
    let document = report.document.ok_or("missing output")?;
    let operation = &document["paths"]["/pets/{id}"]["post"];
    assert_eq!(operation["tags"], json!(["Pets"]));
    assert_eq!(operation["servers"][0]["url"], "https://api.example.com");
    assert_eq!(operation["security"], json!([]));
    assert_eq!(
        operation["requestBody"]["content"]["application/json"]["example"]["id"],
        json!(u64::MAX)
    );
    assert_eq!(
        operation["responses"]["201"]["content"]["application/json"]["example"]["id"],
        json!(u64::MAX)
    );
    let parameters = operation["parameters"]
        .as_array()
        .ok_or("missing parameters")?;
    assert!(parameters.iter().all(|parameter| {
        !["stale", "disabled"]
            .iter()
            .any(|name| parameter["name"] == *name)
    }));
    assert!(
        parameters
            .iter()
            .any(|parameter| parameter["name"] == "id" && parameter["example"] == "123")
    );
    let script = report
        .decisions
        .iter()
        .find(|decision| decision.code == "SCRIPT_NOT_EXECUTED")
        .ok_or("missing script decision")?;
    assert_eq!(script.source.address.pointer, "/items/0/items/0/scripts/0");
    assert!(script.source.offset.is_some());
    Ok(())
}

#[test]
fn v3_body_modes_keep_file_paths_inert_and_report_authored_types()
-> Result<(), Box<dyn std::error::Error>> {
    for body in [
        json!({"type":"none"}),
        json!({"type":"text","content":"hello"}),
        json!({"type":"json","content":"{\"name\":\"Pet\"}","$type":"{\"type\":\"object\",\"required\":[\"name\"]}"}),
        json!({"type":"urlencoded","content":[{"key":"tag","value":"one"},{"key":"tag","value":"two"}]}),
        json!({"type":"formdata","content":[{"key":"file","type":"file","src":"/private/never-read.txt"},{"key":"name","type":"text","value":"Pet"}]}),
        json!({"type":"file","content":{"src":"/private/never-read.bin"}}),
        json!({"type":"graphql","content":{"query":"query { pets { id } }","variables":"{}"}}),
    ] {
        let report = convert(&request(collection(json!([{
            "$kind":"http-request","name":"Body","method":"POST","url":"https://api.example.com/pets","body":body
        }]))))?;
        assert!(report.valid, "{:?}", report.diagnostics);
        if body.get("$type").is_some() {
            assert!(
                report
                    .decisions
                    .iter()
                    .any(|decision| decision.code == "POSTMAN_AUTHORED_TYPE"
                        && decision.source.address.pointer == "/items/0/body/$type")
            );
        }
    }
    Ok(())
}

#[test]
fn v3_inheritance_noauth_and_credentials_keep_required_scopes()
-> Result<(), Box<dyn std::error::Error>> {
    let mut source = collection(json!([
        {"$kind":"http-request","name":"Inherited","method":"GET","url":"https://api.example.com/inherited","auth":{"type":"inherit","credentials":[]}},
        {"$kind":"collection","name":"Anonymous","auth":{"type":"noauth"},"items":[{"$kind":"http-request","name":"Public","method":"GET","url":"https://api.example.com/public"}]}
    ]));
    source["auth"] = json!({"type":"oauth2","credentials":[
        {"key":"grant_type","value":"client_credentials"},
        {"key":"accessTokenUrl","value":"https://auth.example.com/token"},
        {"key":"scope","value":"{{scopes}}"}
    ]});
    let mut input = request(source);
    input
        .config
        .variables
        .insert("scopes".into(), "pets:read pets:write pets:read".into());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("missing output")?;
    let inherited = document["paths"]["/inherited"]["get"]["security"][0]
        .as_object()
        .ok_or("missing security")?;
    assert_eq!(
        inherited.values().next(),
        Some(&json!(["pets:read", "pets:write"]))
    );
    assert_eq!(document["paths"]["/public"]["get"]["security"], json!([]));
    Ok(())
}

#[test]
fn v3_dynamic_auth_never_becomes_anonymous_after_acknowledgement()
-> Result<(), Box<dyn std::error::Error>> {
    let mut source = collection(
        json!([{"$kind":"http-request","name":"Protected","method":"GET","url":"https://api.example.com/pets"}]),
    );
    source["auth"] = json!([{"id":"secret","name":"Conditional","type":"bearer","credentials":[{"key":"token","value":"hidden"}],"rules":[{"key":"url","value":"*.example.com"}]}]);
    let mut input = request(source);
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
    Ok(())
}

#[test]
fn v3_secret_variable_needs_supplied_value_and_never_reads_vault()
-> Result<(), Box<dyn std::error::Error>> {
    let mut source = collection(
        json!([{"$kind":"http-request","name":"Secret","method":"GET","url":"{{base}}/pets"}]),
    );
    source["variables"] = json!([{"key":"base","secret":true,"source":{"provider":"postman","postman":{"type":"local","secretId":"base"}}}]);
    let mut input = request(source);
    assert!(convert(&input).is_err() || convert(&input)?.status == Status::Blocked);
    input
        .config
        .variables
        .insert("base".into(), "https://api.example.com".into());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked);
    Ok(())
}

#[test]
fn v3_non_http_items_remain_blocked_and_missing_schema_references_fail()
-> Result<(), Box<dyn std::error::Error>> {
    let report = convert(&request(collection(
        json!([{"$kind":"grpc-request","name":"RPC","url":"grpc://example.com"}]),
    )))?;
    assert_eq!(report.status, Status::Blocked);
    assert!(
        report.document.ok_or("missing output")?["paths"]
            .as_object()
            .ok_or("missing paths")?
            .is_empty()
    );
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "POSTMAN_PROTOCOL_MAPPING_REQUIRED")
    );
    let error = convert(&request(collection(
        json!([{"$kind":"http-request","name":"Scoped","method":"POST","url":"https://api.example.com/pets","body":{"type":"json","content":"{}","$type":"{\"$ref\":\"#/components/schemas/Pet\"}"}}]),
    ))).err().ok_or("missing schema reference accepted")?;
    assert_eq!(error.code, "SOURCE_INVALID");
    Ok(())
}

#[test]
fn v3_duplicate_secret_variables_and_unsupported_targets_are_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let mut source = collection(json!([]));
    source["variables"] = json!([{"key":"secret","secret":true},{"key":"secret","value":"inline"}]);
    assert_eq!(
        convert(&request(source))
            .err()
            .ok_or("duplicate accepted")?
            .code,
        "POSTMAN_VARIABLE"
    );
    for target in octri_converter::TARGET_VERSIONS {
        let mut input = request(collection(
            json!([{"$kind":"http-request","name":"Get","method":"GET","url":"https://api.example.com/pets","examples":[{"$kind":"http-example","name":"OK","response":{"statusCode":200,"statusText":"OK","body":{"type":"json","content":"{}"}}}]}]),
        ));
        input.config.target_version = Some((*target).into());
        let report = convert(&input)?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
    }
    Ok(())
}

#[test]
fn v3_response_header_annotations_have_physical_review_locations()
-> Result<(), Box<dyn std::error::Error>> {
    let report = convert(&request(collection(json!([{
        "$kind":"http-request","name":"Read","method":"GET","url":"https://api.example.com/pets",
        "examples":[{"$kind":"http-example","name":"OK","response":{
            "statusCode":200,"statusText":"OK",
            "headers":[{"key":"X-Count","value":"3","$type":"integer","description":"Remaining pets"}],
            "body":{"type":"json","content":"{}"}
        }}]
    }]))))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    for (code, key) in [
        ("POSTMAN_PARAMETER_TYPE", "$type"),
        ("POSTMAN_PARAMETER_DESCRIPTION", "description"),
    ] {
        assert!(report.decisions.iter().any(|decision| decision.code == code
            && decision.review_required
            && decision.source.address.pointer
                == format!("/items/0/examples/0/response/headers/0/{key}")
            && decision.source.offset.is_some()));
    }
    Ok(())
}

#[test]
fn v3_auth_docs_survive_root_folder_and_request_inheritance()
-> Result<(), Box<dyn std::error::Error>> {
    let auth = |name: &str| json!({"type":"basic","credentials":[],"id":name,"name":name,"description":format!("{name} credentials")});
    let mut source = collection(json!([
        {"$kind":"http-request","name":"Root","method":"GET","url":"https://api.example.com/root"},
        {"$kind":"collection","name":"Folder","auth":auth("folder"),"items":[{"$kind":"http-request","name":"Folder","method":"GET","url":"https://api.example.com/folder"}]},
        {"$kind":"http-request","name":"Request","method":"GET","url":"https://api.example.com/request","auth":auth("request")}
    ]));
    source["auth"] = auth("root");
    let report = convert(&request(source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("missing output")?;
    let schemes = document["components"]["securitySchemes"]
        .as_object()
        .ok_or("missing schemes")?;
    for (name, auth_at) in [
        ("root", "/auth"),
        ("folder", "/items/1/auth"),
        ("request", "/items/2/auth"),
    ] {
        let security = document["paths"][format!("/{name}")]["get"]["security"][0]
            .as_object()
            .ok_or("missing requirement")?;
        let key = security.keys().next().ok_or("empty requirement")?;
        assert_eq!(schemes[key]["description"], format!("{name} credentials"));
        for field in ["id", "name"] {
            assert!(
                report
                    .decisions
                    .iter()
                    .any(|decision| decision.code == "POSTMAN_AUTH_METADATA"
                        && decision.source.address.pointer == format!("{auth_at}/{field}"))
            );
        }
    }
    Ok(())
}
