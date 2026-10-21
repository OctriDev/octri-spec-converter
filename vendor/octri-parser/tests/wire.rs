#![allow(clippy::unwrap_used)]
use octri_parser::{Address, Input, NoIo, Operation, Options, Request};
use serde_json::{Value, json};

#[test]
fn ascii_report_transport_preserves_unicode_and_exact_numbers() {
    let input = octri_parser::Input {
        uri: "memory://雪/api.json".into(),
        content: Some(r#"{"openapi":"3.1.2","info":{"title":"😀/雪/é/é","version":"1"},"paths":{},"x-data":{"é":"雪","é":"😀","integer":18446744073709551615}}"#.into()),
        generated: false,
    };
    let report = octri_parser::execute(
        octri_parser::Request {
            operation: octri_parser::Operation::Analyze,
            input,
            options: octri_parser::Options::default(),
        },
        &mut octri_parser::NoIo,
    )
    .unwrap();
    let normal: Value =
        serde_json::from_str(&octri_parser::wire::encode_report(Ok(report.clone()))).unwrap();
    let ascii = octri_parser::wire::encode_report_ascii(Ok(report));
    assert!(ascii.is_ascii());
    assert!(ascii.contains("\\ud83d\\ude00"));
    assert_eq!(serde_json::from_str::<Value>(&ascii).unwrap(), normal);
}

fn restore_locations(mut report: Value) -> Value {
    report.as_object_mut().unwrap().remove("wireVersion");
    for source in report["sources"].as_array_mut().unwrap() {
        let uri = source["uri"].clone();
        for (pointer, position) in source["locations"].as_object_mut().unwrap() {
            if position.is_object() {
                continue;
            }
            let mut location = json!({"uri":uri,"pointer":pointer});
            if let Some(values) = position.as_array() {
                for (field, value) in ["line", "column", "offset"].into_iter().zip(values) {
                    if !value.is_null() {
                        location[field] = value.clone();
                    }
                }
            }
            *position = location;
        }
    }
    report
}

#[test]
fn compact_reports_preserve_full_rust_report_values() {
    for version in ["3.0.4", "3.1.2", "3.2.0"] {
        for generated in [false, true] {
            for operation in [
                Operation::Parse,
                Operation::Resolve,
                Operation::Bundle,
                Operation::Dereference,
                Operation::Validate,
                Operation::Inspect,
                Operation::Analyze,
            ] {
                let document = json!({"openapi":version,"info":{"title":"Pets","version":"1"},"paths":{},"components":{"schemas":{"Pet":{"type":"string"},"Alias":{"$ref":"#/components/schemas/Pet"}}}});
                let report = octri_parser::execute(
                    Request {
                        operation,
                        input: Input {
                            uri: "memory://octri/openapi.json".into(),
                            content: Some(document.to_string()),
                            generated,
                        },
                        options: Options::default(),
                    },
                    &mut NoIo,
                )
                .unwrap();
                let expected = serde_json::to_value(&report).unwrap();
                let projected: Value = serde_json::from_str(&octri_parser::wire::encode_document(
                    Ok(octri_parser::DocumentOutput {
                        document: report.document.clone(),
                        graph: report.graph.clone(),
                    }),
                ))
                .unwrap();
                if report.graph.is_some() {
                    assert!(projected["Ok"]["document"].is_null());
                    assert_eq!(projected["Ok"]["graph"]["root"], expected["graph"]["root"]);
                    assert_eq!(
                        projected["Ok"]["graph"]["nodes"],
                        expected["graph"]["nodes"]
                    );
                } else {
                    assert_eq!(projected["Ok"]["document"], expected["document"]);
                }
                assert!(projected["Ok"].get("sources").is_none());
                let encoded: Value =
                    serde_json::from_str(&octri_parser::wire::encode_report(Ok(report))).unwrap();
                assert_eq!(
                    restore_locations(encoded["Ok"].clone()),
                    expected,
                    "{version} {operation:?} generated={generated}"
                );
            }
        }
    }
}

#[test]
fn unusual_locations_and_errors_are_not_changed_by_compact_transport() {
    let mut report = octri_parser::execute(Request {
        operation: Operation::Parse,
        input: Input { uri:"memory://octri/openapi.yaml".into(), content:Some("openapi: 3.1.2\ninfo: {title: Pets, version: '1'}\npaths: {}\nx-data: &value [a, b]\nx-copy: *value\n".into()), generated:false },
        options: Options::default(),
    }, &mut NoIo).unwrap();
    let mut unusual = octri_parser::Location::from(Address {
        uri: "memory://elsewhere".into(),
        pointer: "/different".into(),
    });
    unusual.column = Some(0);
    report.sources[0]
        .locations
        .insert("/unusual".into(), unusual);
    let expected = serde_json::to_value(&report).unwrap();
    let encoded: Value =
        serde_json::from_str(&octri_parser::wire::encode_report(Ok(report))).unwrap();
    assert_eq!(restore_locations(encoded["Ok"].clone()), expected);
    let error = octri_parser::ParserError::new("EXAMPLE", "example", Address::default());
    assert_eq!(
        octri_parser::wire::encode_report(Err(error.clone())),
        octri_parser::wire::encode::<()>(Err(error.clone()))
    );
    assert_eq!(
        octri_parser::wire::encode_document(Err(error.clone())),
        octri_parser::wire::encode::<()>(Err(error))
    );
}
