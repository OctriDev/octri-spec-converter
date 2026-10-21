use octri_parser::{Input, NoIo, Operation, Options, Request, SourceFormat, execute};

#[test]
fn utf8_bom_preserves_source_bytes_and_json_token_locations()
-> Result<(), Box<dyn std::error::Error>> {
    let content = format!(
        "\u{feff}{}",
        r#"{"openapi":"3.2.1","info":{"title":"\uD83D\uDE00","version":"1"},"paths":{}}"#
    );
    let source = octri_parser::parse_source(
        "https://api.example/openapi.json",
        content.clone(),
        &Default::default(),
    )?;
    assert_eq!(source.text, content);
    assert_eq!(source.bytes, content.len());
    assert_eq!(source.document["info"]["title"], "😀");
    let offset = content
        .find(r#""\uD83D\uDE00""#)
        .ok_or("Missing title token")?;
    let location = source
        .locations
        .get("/info/title")
        .ok_or("Missing title location")?;
    assert_eq!(location.offset, Some(offset));
    assert_eq!(location.column, Some(content[..offset].chars().count() + 1));
    assert!(!source.generated);
    for uri in ["memory://project/openapi.yaml", "memory://project/paste"] {
        let auto = octri_parser::parse_source(uri, content.clone(), &Default::default())?;
        assert_eq!(auto.document["info"]["title"], "😀");
        assert_eq!(auto.text, content);
    }

    let limits = octri_parser::Limits {
        max_document_bytes: content.len() - 1,
        ..Default::default()
    };
    let error = octri_parser::parse_source("https://api.example/openapi.json", content, &limits)
        .err()
        .ok_or("BOM bytes escaped the source limit")?;
    assert_eq!(error.diagnostics[0].code, "LIMIT_DOCUMENT_BYTES");
    Ok(())
}

#[test]
fn bom_does_not_make_malformed_json_valid() -> Result<(), Box<dyn std::error::Error>> {
    let error = octri_parser::parse_source(
        "https://api.example/openapi.json",
        "\u{feff}{\"value\":1,}".into(),
        &Default::default(),
    )
    .err()
    .ok_or("Malformed JSON was accepted")?;
    assert_eq!(error.diagnostics[0].code, "INVALID_JSON");
    for (content, line, column) in [("\u{feff}{!}", 1, 3), ("\u{feff}{\n!}", 2, 1)] {
        let error = octri_parser::parse_source(
            "memory://test/api.json",
            content.into(),
            &Default::default(),
        )
        .err()
        .ok_or("Malformed JSON was accepted")?;
        assert_eq!(error.diagnostics[0].location.line, Some(line));
        assert_eq!(error.diagnostics[0].location.column, Some(column));
    }
    Ok(())
}

#[test]
fn bom_keeps_duplicate_and_reserved_key_guards() -> Result<(), Box<dyn std::error::Error>> {
    let duplicate = octri_parser::parse_source(
        "memory://test/api.json",
        "\u{feff}{\"value\":1,\"value\":2}".into(),
        &Default::default(),
    )
    .err()
    .ok_or("Duplicate key was accepted")?;
    assert_eq!(duplicate.diagnostics[0].code, "DUPLICATE_KEY");
    let reserved = octri_parser::parse_source(
        "memory://test/api.json",
        "\u{feff}{\"$serde_json::private::Number\":\"1\"}".into(),
        &Default::default(),
    )
    .err()
    .ok_or("Reserved key was accepted")?;
    assert_eq!(reserved.diagnostics[0].code, "UNSAFE_KEY");
    assert_eq!(reserved.diagnostics[0].location.offset, Some(4));
    assert_eq!(reserved.diagnostics[0].location.column, Some(3));
    Ok(())
}

#[test]
fn generated_object_transport_does_not_accept_a_bom() -> Result<(), Box<dyn std::error::Error>> {
    let request = Request {
        operation: Operation::Parse,
        input: Input {
            uri: "memory://test/api.json".into(),
            content: Some("\u{feff}{}".into()),
            generated: true,
        },
        options: Options::default(),
    };
    let error = execute(request, &mut NoIo)
        .err()
        .ok_or("Generated transport accepted BOM")?;
    assert_eq!(error.diagnostics[0].code, "INVALID_JSON");
    Ok(())
}

#[test]
fn explicit_root_encoding_retains_uri_and_external_resource_hints()
-> Result<(), Box<dyn std::error::Error>> {
    let uri = "memory://project/root.json";
    let yaml = "openapi: 3.1.2\ninfo: {title: Encoding, version: '1'}\npaths: {}\ncomponents:\n  schemas:\n    Pet: {$ref: 'pet.json'}\n";
    let mut options = Options {
        root_format: SourceFormat::Yaml,
        ..Default::default()
    };
    options.files.insert(
        "pet.json".into(),
        "{\"type\":\"number\",\"example\":0.100000000000000005}".into(),
    );
    let request = Request {
        operation: Operation::Analyze,
        input: Input {
            uri: uri.into(),
            content: Some(yaml.into()),
            generated: false,
        },
        options,
    };
    let result = execute(request.clone(), &mut NoIo)?;
    assert!(result.valid, "{:?}", result.diagnostics);
    assert_eq!(result.registry.root_uri, uri);
    assert!(
        result
            .sources
            .iter()
            .any(|source| source.uri == "memory://project/pet.json")
    );
    let mut strict_hint = request.clone();
    strict_hint.options.root_format = SourceFormat::Auto;
    let hinted = execute(strict_hint, &mut NoIo)?;
    assert!(!hinted.valid);
    assert!(hinted.diagnostics.iter().any(|d| d.code == "INVALID_JSON"));
    let mut bad_dependency = request;
    bad_dependency
        .options
        .files
        .insert("pet.json".into(), "type: number".into());
    let result = execute(bad_dependency, &mut NoIo)?;
    assert!(!result.valid);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == "INVALID_JSON" && d.location.address.uri.ends_with("pet.json"))
    );
    Ok(())
}
