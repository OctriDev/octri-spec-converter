use octri_converter::{Config, ConversionRequest, convert};
use octri_parser::Input;
use serde_json::{Value, json};

fn node_count(value: &Value) -> usize {
    1 + match value {
        Value::Object(map) => map.values().map(node_count).sum::<usize>(),
        Value::Array(values) => values.iter().map(node_count).sum::<usize>(),
        _ => 0,
    }
}

#[test]
fn internal_envelopes_do_not_consume_physical_source_allowances()
-> Result<(), Box<dyn std::error::Error>> {
    let document = json!({"asyncapi":"3.0.0","info":{"title":"Events","version":"1"},
        "x-source-data":(0..500).collect::<Vec<_>>(),
        "channels":{"events":{"address":"events","messages":{"event":{"payload":{"type":"string"}}}}},
        "operations":{"consume":{"action":"receive","channel":{"$ref":"#/channels/events"}}}});
    let content = document.to_string();
    let mut config = Config::default();
    config.limits.max_documents = 1;
    config.limits.max_document_bytes = content.len();
    config.limits.max_total_bytes = content.len();
    config.limits.max_nodes = node_count(&document);
    config.http_mappings.insert("consume".into(),serde_json::from_value(json!({"method":"POST","path":"/events","server":"https://api.example.com","responseStatus":"202","security":[],"payloadLocation":"request"}))?);
    let input = ConversionRequest {
        input: Input {
            uri: "memory://limits/events.json".into(),
            content: Some(content),
            generated: false,
        },
        files: Default::default(),
        config,
    };
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.manifest.files.len(), 1);
    assert_eq!(
        serde_json::to_value(&report.original)?,
        serde_json::to_value(&input)?
    );
    let mut extra_file = input.clone();
    extra_file.files.insert("extra.json".into(), "{}".into());
    assert_eq!(
        convert(&extra_file)
            .err()
            .ok_or("extra file was accepted")?
            .code,
        "LIMIT_INPUT"
    );
    let mut extra_byte = input;
    extra_byte
        .input
        .content
        .as_mut()
        .ok_or("missing input")?
        .push(' ');
    assert_eq!(
        convert(&extra_byte)
            .err()
            .ok_or("extra byte was accepted")?
            .code,
        "LIMIT_INPUT"
    );
    Ok(())
}
