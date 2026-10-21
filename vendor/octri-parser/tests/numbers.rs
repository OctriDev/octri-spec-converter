use octri_parser::{Limits, numbers, parse_source, serialize, serialize_yaml};
use serde_json::Value;

#[test]
fn decimal_tokens_survive_json_yaml_and_serialization_without_binary64_rounding()
-> Result<(), Box<dyn std::error::Error>> {
    for token in [
        "0.100000000000000005",
        "9007199254740993.25",
        "18446744073709551616",
        "1e400",
        "1e9223372036854775808",
    ] {
        for text in [
            format!("{{\"value\":{token}}}"),
            format!("value: {token}\n"),
        ] {
            let parsed = parse_source("memory://numbers/source", text, &Limits::default())?;
            let number = parsed.document["value"]
                .as_number()
                .ok_or("numeric type lost")?;
            assert_eq!(
                numbers::compare_text(&number.to_string(), token),
                std::cmp::Ordering::Equal
            );
            let json = serialize(&parsed.document, &Limits::default())?;
            if token == "0.100000000000000005"
                || token == "9007199254740993.25"
                || token == "18446744073709551616"
            {
                assert!(json.contains(token), "Numeric token rounded: {json}");
            }
            let yaml = serialize_yaml(&parsed.document, &Limits::default())?;
            let reparsed = parse_source("memory://numbers/output.yaml", yaml, &Limits::default())?;
            assert!(numbers::equivalent(&parsed.document, &reparsed.document));
        }
    }
    Ok(())
}

#[test]
fn exact_number_transport_preserves_limits_and_refuses_reserved_object_spoofing()
-> Result<(), Box<dyn std::error::Error>> {
    let limits = Limits {
        max_nodes: 2,
        max_depth: 1,
        ..Default::default()
    };
    let document = parse_source(
        "memory://numbers/test.json",
        "{\"value\":0.100000000000000005}".into(),
        &limits,
    )?;
    assert!(document.document["value"].is_number());
    assert!(
        parse_source(
            "memory://numbers/test.json",
            "{\"value\":0.1,\"extra\":1}".into(),
            &limits
        )
        .is_err()
    );
    for text in [
        r#"{"$serde_json::private::Number":"1"}"#,
        r#"{"\u0024serde_json::private::Number":"1"}"#,
        "'$serde_json::private::Number': '1'",
        r#"{"safe":{"$serde_json::private::Number":"1"}}"#,
    ] {
        assert!(parse_source("memory://numbers/test", text.into(), &Limits::default()).is_err());
    }
    let literal = parse_source(
        "memory://numbers/test.json",
        r#"{"value":"$serde_json::private::Number"}"#.into(),
        &Limits::default(),
    )?;
    assert!(matches!(literal.document["value"], Value::String(_)));
    assert!(
        parse_source(
            "memory://numbers/test.json",
            r#"{"value":1,"value":2}"#.into(),
            &Limits::default()
        )
        .is_err()
    );
    assert!(
        parse_source(
            "memory://numbers/test.json",
            r#"{"__proto__":{}}"#.into(),
            &Limits::default()
        )
        .is_err()
    );
    Ok(())
}
