//! The hosted parser has its own versioned envelope and no ambient I/O.
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn execute(input: Value) -> Result<(i32, Value), Box<dyn std::error::Error>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_octri-spec-parse"))
        .arg("execute")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("stdin missing")?
        .write_all(input.to_string().as_bytes())?;
    let output = child.wait_with_output()?;
    Ok((
        output.status.code().ok_or("worker terminated")?,
        serde_json::from_slice(&output.stdout)?,
    ))
}

fn request() -> Value {
    json!({"abiVersion":1,"request":{
        "operation":"analyze",
        "input":{"uri":"memory://host/api.json","content":json!({
            "openapi":"3.2.1", "info":{"title":"Hosted parser","version":"1"},
            "paths":{"/events":{"$ref":"events.json"}}
        }).to_string()},
        "options":{"validate":{"mode":"strict"},"files":{
            "events.json":json!({"additionalOperations":{
                "copy":{"description":"Keep exact wire spelling", "x-evidence":{"$ref":"literal"},"responses":{"204":{"description":"OUTPUT_LIMIT remains literal data"}}}
            }}).to_string()
        }}
    }})
}

#[test]
fn hosted_parser_retains_case_metadata_and_external_source_addresses()
-> Result<(), Box<dyn std::error::Error>> {
    let (code, report) = execute(request())?;
    assert_eq!(code, 0);
    assert_eq!(report["Ok"]["valid"], true, "{report}");
    let operation = &report["Ok"]["inspection"]["operations"][0];
    assert_eq!(operation["method"], "copy");
    assert_eq!(operation["description"], "Keep exact wire spelling");
    assert_eq!(operation["extensions"]["x-evidence"]["$ref"], "literal");
    assert_eq!(operation["location"]["uri"], "memory://host/events.json");
    Ok(())
}

#[test]
fn hosted_parser_accepts_checked_retrieval_aliases_and_original_bom_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    let content = format!(
        "\u{feff}{}",
        json!({
            "openapi":"3.2.1", "info":{"title":"Redirected sources","version":"1"},
            "paths":{"/events":{"$ref":"https://old.example/events.json"}}
        })
    );
    let input = json!({"abiVersion":1,"request":{
        "operation":"analyze",
        "input":{"uri":"https://api.example/openapi.json","content":content},
        "options":{"retrievalAliases":{
            "https://old.example/events.json":"https://cdn.example/v2/events.json"
        },"files":{
            "https://cdn.example/v2/events.json":json!({"additionalOperations":{
                "copy":{"responses":{"200":{"$ref":"response.json"}}}
            }}).to_string(),
            "https://cdn.example/v2/response.json":json!({"description":"Final base retained"}).to_string()
        }}
    }});
    let (code, report) = execute(input)?;
    assert_eq!(code, 0);
    assert_eq!(report["Ok"]["valid"], true, "{report}");
    assert!(
        report["Ok"]["sources"]
            .as_array()
            .is_some_and(|sources| sources.iter().any(|source| source["text"] == content))
    );
    let operation = &report["Ok"]["inspection"]["operations"][0];
    assert_eq!(operation["method"], "copy");
    assert_eq!(
        operation["location"]["uri"],
        "https://cdn.example/v2/events.json"
    );
    assert_eq!(
        operation["responses"]["200"]["description"],
        "Final base retained"
    );
    assert_eq!(
        report["Ok"]["registry"]["retrievalAliases"]["https://old.example/events.json"],
        "https://cdn.example/v2/events.json"
    );
    Ok(())
}

#[test]
fn parser_transport_rejects_abi_changes_expansion_and_host_capabilities()
-> Result<(), Box<dyn std::error::Error>> {
    let mut expanded = request();
    expanded["request"]["options"]["limits"] = json!({"maxNodes":2_000_000});
    let (code, output) = execute(expanded)?;
    assert_eq!(code, 0, "{output}");
    assert_eq!(output["Ok"]["valid"], true, "{output}");

    for (pointer, value, expected) in [
        ("/abiVersion", json!(2), "ABI_VERSION"),
        (
            "/request/options/limits",
            json!({"maxNodes":2_000_001}),
            "INVALID_OPTIONS",
        ),
        (
            "/request/options/resolve",
            json!({"http":true}),
            "HOST_CAPABILITY_DENIED",
        ),
        (
            "/request/options/resolve",
            json!({"fileRoot":"/"}),
            "HOST_CAPABILITY_DENIED",
        ),
    ] {
        let mut input = request();
        // The options object is caller-owned test data, not a JSON Patch engine.
        let (parent, key) = pointer.rsplit_once('/').ok_or("invalid test pointer")?;
        input.pointer_mut(parent).ok_or("missing test parent")?[key] = value;
        let (code, output) = execute(input)?;
        assert_eq!(code, 2);
        assert_eq!(
            output["Err"]["diagnostics"][0]["code"], expected,
            "{output}"
        );
    }
    let mut input = request();
    input["unexpected"] = json!(true);
    assert_eq!(
        execute(input)?.1["Err"]["diagnostics"][0]["code"],
        "INVALID_REQUEST"
    );
    Ok(())
}

#[test]
fn resource_uris_never_authorize_implicit_reads() -> Result<(), Box<dyn std::error::Error>> {
    for reference in [
        "file:///etc/passwd",
        "http://127.0.0.1/private",
        "https://example.invalid/missing",
    ] {
        let mut input = request();
        input["request"]["options"]["files"] = json!({});
        input["request"]["input"]["content"] = json!(
            json!({
                "openapi":"3.2.1", "info":{"title":"Offline","version":"1"},
                "paths":{"/events":{"$ref":reference}}
            })
            .to_string()
        );
        let (code, report) = execute(input)?;
        assert_eq!(code, 0);
        assert_eq!(report["Ok"]["valid"], false, "{report}");
        assert!(
            report["Ok"]["diagnostics"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        );
    }
    Ok(())
}
