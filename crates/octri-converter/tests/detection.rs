use octri_converter::{Config, ConversionRequest, InputFormat, convert, detect};
use octri_parser::Input;

fn request(content: &str, format: InputFormat) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://pasted/source.yaml".into(),
            content: Some(content.into()),
            generated: false,
        },
        files: Default::default(),
        config: Config {
            input_format: format,
            ..Default::default()
        },
    }
}

#[test]
fn explicit_text_formats_reach_their_own_syntax_readers() -> Result<(), Box<dyn std::error::Error>>
{
    for (format, source, operations) in [
        (
            InputFormat::Graphql,
            "# Public query contract\n\"\"\"API query\"\"\"\ntype Query { ping: String }",
            1,
        ),
        (
            InputFormat::Protobuf,
            "// Proto2 permits an omitted syntax declaration.\nmessage Input {} message Output {} service Ping { rpc Call(Input) returns (Output); }",
            1,
        ),
        (
            InputFormat::Curl,
            "'curl' 'https://api.example.com/ping'",
            0,
        ),
    ] {
        let input = request(source, format);
        assert_eq!(detect(&input)?, format);
        let report = convert(&input)?;
        assert_eq!(report.mapping_requests.len(), operations);
        if format == InputFormat::Curl {
            assert!(report.valid, "{:?}", report.diagnostics);
            assert!(
                report
                    .document
                    .and_then(|value| value.pointer("/paths/~1ping/get").cloned())
                    .is_some()
            );
        }
    }
    Ok(())
}

#[test]
fn explicit_selection_does_not_override_conflicting_declarations() {
    let document = r#"{"openapi":"3.1.2","info":{"title":"API","version":"1"},"paths":{}}"#;
    for format in [
        InputFormat::Graphql,
        InputFormat::Protobuf,
        InputFormat::Curl,
    ] {
        assert_eq!(
            detect(&request(document, format))
                .err()
                .map(|error| error.code),
            Some("FORMAT_MISMATCH".into())
        );
    }
    assert!(convert(&request("this is not GraphQL", InputFormat::Graphql)).is_err());
    assert!(convert(&request("this is not Protobuf", InputFormat::Protobuf)).is_err());
    assert!(convert(&request("echo unsafe", InputFormat::Curl)).is_err());
}
