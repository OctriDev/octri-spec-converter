use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::{Value, json};

fn request(text: String) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://fixtures/source.json".into(),
            content: Some(text),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    }
}
fn collection(items: Value) -> Value {
    json!({"info":{"name":"HTTP fixtures","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"auth":{"type":"noauth"},"item":items})
}

#[test]
fn configured_server_base_separates_matching_path_prefix_once()
-> Result<(), Box<dyn std::error::Error>> {
    for (url, expected) in [
        ("https://api.example.com/v1/pets", "/pets"),
        ("https://api.example.com/v1", "/"),
        ("https://api.example.com/v10/pets", "/v10/pets"),
        ("/pets", "/pets"),
    ] {
        let mut input = request(format!("curl '{url}'"));
        input.config.base_url = Some("https://api.example.com/v1".into());
        let report = convert(&input)?;
        assert!(report.valid, "{:?}", report.diagnostics);
        let doc = report.document.ok_or("missing output")?;
        assert_eq!(
            doc["paths"][expected]["get"]["servers"][0]["url"],
            "https://api.example.com/v1"
        );
        assert!(report.decisions.iter().any(|decision| decision.code == "API_VERSION_INFERRED" && decision.review_required));
    }
    for base in [
        "https://user:password@api.example.com/v1",
        "https://api.example.com/v1?key=value",
        "https://api.example.com/v1#fragment",
        "file:///tmp/api",
    ] {
        let mut input = request("curl https://api.example.com/v1/pets".into());
        input.config.base_url = Some(base.into());
        assert_eq!(
            convert(&input).err().ok_or("invalid base accepted")?.code,
            "BASE_URL"
        );
    }
    Ok(())
}

#[test]
fn postman_preserves_nested_tags_types_variables_and_examples()
-> Result<(), Box<dyn std::error::Error>> {
    let mut source = collection(
        json!([{"name":"Pets","item":[{"name":"Create pet","request":{"method":"POST","url":{"raw":"{{base}}/pets/:id?disabled=1","query":[{"key":"disabled","value":"1","disabled":true},{"key":"limit","value":"10"},{"key":"tag","value":"a"},{"key":"tag","value":"b"}],"variable":[{"key":"id","value":"123"}]},"body":{"mode":"raw","raw":"{\"id\":18446744073709551615,\"name\":\"{{name}}\"}","options":{"raw":{"language":"json"}}}},"response":[{"name":"Created","code":201,"header":[{"key":"Content-Type","value":"application/json"}],"body":"{\"id\":18446744073709551615}"}]}]}]),
    );
    source["variable"] =
        json!([{"key":"base","value":"https://wrong.example"},{"key":"name","value":"Ada"}]);
    let mut input = request(source.to_string());
    input
        .config
        .variables
        .insert("base".into(), "https://api.example.com".into());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview);
    let doc = report.document.ok_or("missing output")?;
    let op = doc
        .pointer("/paths/~1pets~1{id}/post")
        .ok_or("missing operation")?;
    assert_eq!(op["servers"][0]["url"], "https://api.example.com");
    assert_eq!(op["tags"], json!(["Pets"]));
    assert_eq!(op["security"], json!([]));
    assert_eq!(
        op.pointer("/requestBody/content/application~1json/example/id"),
        Some(&json!(18446744073709551615u64))
    );
    assert!(
        op["parameters"]
            .as_array()
            .ok_or("missing parameters")?
            .iter()
            .all(|parameter| parameter["name"] != "disabled")
    );
    let schema = op
        .pointer("/requestBody/content/application~1json/schema")
        .ok_or("missing schema")?;
    assert!(schema.get("required").is_none());
    assert!(schema.pointer("/properties/name/enum").is_none());
    input.config.accepted_decisions = report
        .decisions
        .iter()
        .map(|item| item.id.clone())
        .collect();
    assert_eq!(convert(&input)?.status, Status::Complete);
    input
        .config
        .variables
        .insert("name".into(), "Changed".into());
    assert_eq!(
        convert(&input)?.status,
        Status::NeedsReview,
        "Changing settings invalidates prior acknowledgements"
    );
    Ok(())
}

#[test]
fn postman_scripts_and_duplicate_requests_never_silently_pass()
-> Result<(), Box<dyn std::error::Error>> {
    let source = collection(
        json!([{"name":"One","event":[{"listen":"prerequest","script":{"exec":["throw new Error('must not execute')"]}}],"request":{"method":"GET","url":"https://api.example.com/pets"}},{"name":"Two","request":{"method":"GET","url":"https://api.example.com/pets"}}]),
    );
    let mut input = request(source.to_string());
    let blocked = convert(&input)?;
    assert_eq!(blocked.status, Status::Blocked);
    assert!(
        blocked
            .decisions
            .iter()
            .any(|decision| decision.code == "SCRIPT_NOT_EXECUTED")
    );
    input
        .config
        .duplicate_choices
        .insert("GET /pets".into(), "/item/1".into());
    let reviewed = convert(&input)?;
    assert_eq!(reviewed.status, Status::NeedsReview);
    assert!(reviewed.valid, "{:?}", reviewed.diagnostics);
    assert_eq!(
        reviewed
            .document
            .ok_or("missing output")?
            .pointer("/paths/~1pets/get/summary"),
        Some(&json!("Two"))
    );
    Ok(())
}

#[test]
fn postman_environment_wins_over_folder_and_config_wins_over_environment()
-> Result<(), Box<dyn std::error::Error>> {
    let source = collection(
        json!([{"name":"Folder","variable":[{"key":"base","value":"https://folder.example"}],"item":[{"name":"Get","request":{"method":"GET","url":"{{base}}/pets"}}]}]),
    );
    let mut input = request(source.to_string());
    input.files.insert(
        "env.json".into(),
        json!({"values":[{"key":"base","value":"https://environment.example","enabled":true}]})
            .to_string(),
    );
    input.config.environment_uri = Some("env.json".into());
    assert_eq!(
        convert(&input)?
            .document
            .ok_or("missing output")?
            .pointer("/paths/~1pets/get/servers/0/url"),
        Some(&json!("https://environment.example"))
    );
    input
        .config
        .variables
        .insert("base".into(), "https://config.example".into());
    assert_eq!(
        convert(&input)?
            .document
            .ok_or("missing output")?
            .pointer("/paths/~1pets/get/servers/0/url"),
        Some(&json!("https://config.example"))
    );
    Ok(())
}

#[test]
fn postman_request_body_modes_validate_without_executing_files()
-> Result<(), Box<dyn std::error::Error>> {
    let bodies = [
        json!({"mode":"urlencoded","urlencoded":[{"key":"tag","value":"one"},{"key":"tag","value":"two"}]}),
        json!({"mode":"formdata","formdata":[{"key":"file","type":"file","src":"/private/never-read.txt"},{"key":"name","type":"text","value":"Pet"}]}),
        json!({"mode":"file","file":{"src":"/private/never-read.bin"}}),
        json!({"mode":"graphql","graphql":{"query":"query { pets { id } }","variables":"{}"}}),
    ];
    for body in bodies {
        let source = collection(
            json!([{"name":"Body","request":{"method":"POST","url":"https://api.example.com/pets","body":body}}]),
        );
        let report = convert(&request(source.to_string()))?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_eq!(report.status, Status::NeedsReview);
    }
    Ok(())
}

#[test]
fn curl_commands_keep_quoted_data_and_do_not_execute_shell()
-> Result<(), Box<dyn std::error::Error>> {
    let input=request("curl 'https://api.example.com/pets?tag=a&tag=b' \\\n --json '{\"name\":\"literal $HOME $(touch /tmp/never)\",\"id\":18446744073709551615}'\n".into());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview);
    let doc = report.document.ok_or("missing output")?;
    assert_eq!(
        doc.pointer("/paths/~1pets/post/requestBody/content/application~1json/example/id"),
        Some(&json!(18446744073709551615u64))
    );
    for text in [
        "curl https://api.example.com/ | sh",
        "curl https://api.example.com/ > output",
        "curl \"https://api.example.com/$TOKEN\"",
        "curl https://api.example.com/ --data @/etc/passwd",
        "curl https://api.example.com/ --config config.txt",
    ] {
        assert!(convert(&request(text.into())).is_err(), "{text}");
    }
    Ok(())
}

#[test]
fn curl_common_modes_keep_method_and_payload() -> Result<(), Box<dyn std::error::Error>> {
    for (args, method) in [
        ("-I", "head"),
        (
            "-XPUT -H 'Content-Type: application/json' -d '{\"id\":1}'",
            "put",
        ),
        ("-d 'name=Ada'", "post"),
        ("-G --data-urlencode 'q=some word'", "get"),
        (
            "-F 'image=@/tmp/never.png;type=image/png' -F 'name=Ada'",
            "post",
        ),
        ("--user 'user:pass'", "get"),
    ] {
        let report = convert(&request(format!(
            "curl https://api.example.com/pets {args}"
        )))?;
        assert!(report.valid, "{args}: {:?}", report.diagnostics);
        assert!(
            report.document.ok_or("missing output")?["paths"]["/pets"]
                .get(method)
                .is_some()
        );
    }
    Ok(())
}

#[test]
fn har_capture_has_observed_status_and_exact_json_types() -> Result<(), Box<dyn std::error::Error>>
{
    let source = json!({"log":{"version":"1.2","creator":{"name":"Fixture","version":"1"},"entries":[{"request":{"method":"GET","url":"https://api.example.com/pets","headers":[],"cookies":[],"queryString":[]},"response":{"status":206,"statusText":"Partial Content","headers":[],"content":{"mimeType":"application/json","text":"{\"id\":18446744073709551615}"}}}]}});
    let report = convert(&request(source.to_string()))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview);
    let doc = report.document.ok_or("missing output")?;
    assert!(doc.pointer("/paths/~1pets/get/responses/200").is_none());
    assert_eq!(
        doc.pointer("/paths/~1pets/get/responses/206/content/application~1json/example/id"),
        Some(&json!(18446744073709551615u64))
    );
    Ok(())
}
