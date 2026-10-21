use octri_parser::{Input, NoIo, Options, bundle_sources};

#[test]
fn bundle_keeps_imported_physical_origins_and_literal_payloads()
-> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options::default();
    options.files.insert("memory://api/pet.json".into(), r#"{"type":"object","properties":{"id":{"type":"integer"}},"example":{"$ref":"not-a-reference"}}"#.into());
    let input = Input { uri: "memory://api/openapi.json".into(), content: Some(r#"{"openapi":"3.1.2","info":{"title":"Pets","version":"1"},"paths":{},"components":{"schemas":{"Pet":{"$ref":"pet.json"}}}}"#.into()), generated: false };
    let result = bundle_sources(input, options, &mut NoIo)?;
    assert_eq!(result.references.len(), 1);
    let edge = &result.references[0];
    assert_eq!(edge.reference, "pet.json");
    assert!(edge.resolved);
    assert_eq!(edge.from.address.uri, "memory://api/openapi.json");
    assert_eq!(edge.from.address.pointer, "/components/schemas/Pet/$ref");
    let target = &edge.target.as_ref().ok_or("resolved target")?.address;
    assert_eq!(target.uri, "memory://api/pet.json");
    assert_eq!(target.pointer, "");
    let origin = &result.origins["/components/schemas/OctriExternal1/properties/id"];
    assert_eq!(origin.uri, "memory://api/pet.json");
    assert_eq!(origin.pointer, "/properties/id");
    assert_eq!(
        result
            .document
            .pointer("/components/schemas/OctriExternal1/example/$ref")
            .and_then(serde_json::Value::as_str),
        Some("not-a-reference")
    );
    Ok(())
}
