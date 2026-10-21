//! Selected schemas retain enclosing resources, physical includes and XML names.
use octri_converter::{ConversionRequest, convert};
use serde_json::{Value, json};

const XML: &str = r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema" xmlns:t="urn:items" targetNamespace="urn:items"><x:element name="Text" type="x:string"/><x:element name="Count" type="x:integer"/><x:complexType name="Payload"><x:all><x:element name="value" type="x:boolean"/></x:all></x:complexType></x:schema>"#;

fn request(include: &str, source: &str, xml: bool) -> Result<ConversionRequest, serde_json::Error> {
    serde_json::from_value(json!({
        "input":{"uri":"memory://raml/api.raml","content":format!("#%RAML 1.0\ntitle: Selected schemas\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n  payload: !include traits/body.raml\n/items:\n  post:\n    is: [payload]\n    responses:\n      204: {{description: Empty}}\n")},
        "files":{
            "traits/body.raml":format!("body:\n  application/{}:\n    type: !include {include}\n",if xml {"xml"} else {"json"}),
            "traits/schema.xsd":source,
            "traits/schema.json":source
        }
    }))
}

fn validator(
    document: &Value,
    media: &str,
) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let mut registry = jsonschema::Registry::new();
    for schema in document["components"]["schemas"]
        .as_object()
        .ok_or("schemas")?
        .values()
    {
        if let Some(id) = schema
            .get("$id")
            .or_else(|| schema.get("id"))
            .and_then(Value::as_str)
        {
            registry = registry.add(id, jsonschema::Resource::from_contents(schema.clone()))?;
        }
    }
    let registry = registry
        .add(
            "memory://output/api.json",
            jsonschema::Resource::from_contents(document.clone()),
        )?
        .prepare()?;
    Ok(jsonschema::options().with_registry(&registry).build(&json!({"$ref":format!(
        "memory://output/api.json#/paths/~1items/post/requestBody/content/application~1{media}/schema"
    )}))?)
}

#[test]
fn json_selection_keeps_enclosing_dialects_identifiers_and_sibling_references()
-> Result<(), Box<dyn std::error::Error>> {
    for (dialect, definitions) in [
        ("http://json-schema.org/draft-04/schema#", "definitions"),
        ("http://json-schema.org/draft-07/schema#", "definitions"),
        ("https://json-schema.org/draft/2020-12/schema", "$defs"),
    ] {
        let schema = json!({"$schema":dialect, definitions:{
            "Payload":{"type":"object","properties":{"value":{"$ref":format!("#/{definitions}/Count")}},"required":["value"]},
            "Count":{"type":"integer","minimum":2}
        }}).to_string();
        let report = convert(&request(
            &format!("schema.json#/{definitions}/Payload"),
            &schema,
            false,
        )?)?;
        assert!(report.valid, "{:?}", report.diagnostics);
        let document = report.document.ok_or("output")?;
        let check = validator(&document, "json")?;
        assert!(check.is_valid(&json!({"value":2})));
        assert!(!check.is_valid(&json!({"value":1})));
        assert!(!check.is_valid(&json!("not payload")));
    }
    let schema = json!({"$schema":"https://json-schema.org/draft/2020-12/schema","$id":"https://contracts.test/root.json","$defs":{
        "Payload":{"$id":"nested/payload.json","type":"object","properties":{"value":{"$ref":"value.json"}},"required":["value"]}
    }}).to_string();
    let mut input = request("schema.json#/$defs/Payload", &schema, false)?;
    input.files.insert(
        "https://contracts.test/nested/value.json".into(),
        "{\"type\":\"integer\",\"minimum\":2}".into(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("output")?;
    assert!(validator(&document, "json")?.is_valid(&json!({"value":2})));
    assert!(!validator(&document, "json")?.is_valid(&json!({"value":1})));
    Ok(())
}

#[test]
fn fragment_dialect_and_rejection_locations_are_not_inferred_from_selected_values()
-> Result<(), Box<dyn std::error::Error>> {
    for schema in [
        r#"{"$schema":"https://example.test/unknown","$defs":{"Payload":{"type":"string"}}}"#,
        r#"{"$schema":7,"$defs":{"Payload":{"type":"string"}}}"#,
    ] {
        let error = convert(&request("schema.json#/$defs/Payload", schema, false)?)
            .err()
            .ok_or("unsupported enclosing dialect accepted")?;
        assert_eq!(error.code, "RAML_SCHEMA_FRAGMENT");
        assert_eq!(
            error.diagnostics[0].location.address.uri,
            "memory://raml/traits/body.raml"
        );
        assert_eq!(error.diagnostics[0].location.line, Some(3));
    }
    let mut input = request("schema.xsd#Missing", XML, true)?;
    let error = convert(&input).err().ok_or("missing selector accepted")?;
    assert_eq!(
        error.diagnostics[0].location.address.pointer,
        "/body/application~1xml/type"
    );
    input.input.content = input.input.content.map(|text| {
        text.replace("#%RAML 1.0", "#%RAML 0.8")
            .replace("  payload:", "  - payload:")
    });
    input.files.insert(
        "traits/body.raml".into(),
        "body:\n  application/xml:\n    schema: !include schema.xsd#Text\n".into(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(validator(&report.document.ok_or("output")?, "xml")?.is_valid(&json!("text")));
    Ok(())
}

#[test]
fn json_pointer_escapes_unicode_booleans_and_original_bytes_survive()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = "\u{feff}{\"$defs\":{\"猫/~\":{\"const\":\"Code\"},\"Never\":false}}";
    for (fragment, pass) in [("#/%24defs/%E7%8C%AB~1~0", true), ("#/$defs/Never", false)] {
        let input = request(&format!("schema.json{fragment}"), schema, false)?;
        let report = convert(&input)?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert!(report.manifest.files.iter().any(|file| file.uri
            == "memory://raml/traits/schema.json"
            && file.bytes == schema.len()));
        let document = report.document.ok_or("output")?;
        assert_eq!(validator(&document, "json")?.is_valid(&json!("Code")), pass);
        assert!(!validator(&document, "json")?.is_valid(&json!("other")));
    }
    let report = convert(&request("schema.json#", "false", false)?)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(!validator(&report.document.ok_or("output")?, "json")?.is_valid(&json!(null)));
    Ok(())
}

#[test]
fn xml_element_and_complex_type_select_only_authored_contract()
-> Result<(), Box<dyn std::error::Error>> {
    for (name, accepted, rejected) in [
        ("Text", json!("ok"), json!(12)),
        ("Count", json!(12), json!("wrong")),
        ("Payload", json!({"value":true}), json!({"value":"wrong"})),
    ] {
        let report = convert(&request(&format!("schema.xsd#{name}"), XML, true)?)?;
        assert!(report.valid, "{:?}", report.diagnostics);
        let type_review = report.decisions.iter().any(|decision| {
            decision.code == "RAML_XSD_TYPE_SERIALIZATION" && decision.review_required
        });
        assert_eq!(type_review, name == "Payload");
        let document = report.document.ok_or("output")?;
        let check = validator(&document, "xml")?;
        assert!(check.is_valid(&accepted));
        assert!(!check.is_valid(&rejected));
    }
    let types_only = r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema"><x:complexType name="Empty"/></x:schema>"#;
    assert!(convert(&request("schema.xsd#Empty", types_only, true)?)?.valid);
    Ok(())
}

#[test]
fn distinct_xml_selectors_share_projection_budget_without_sharing_roots()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request("schema.xsd#Text", XML, true)?;
    input.files.remove("traits/schema.json");
    input.input.content.as_mut().ok_or("root")?.push_str("/counts:\n  post:\n    body:\n      application/xml:\n        type: !include traits/schema.xsd#Count\n    responses:\n      204: {description: Empty}\n");
    input.config.limits.max_documents = 3;
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("output")?;
    assert!(validator(&document, "xml")?.is_valid(&json!("text")));
    assert!(!validator(&document, "xml")?.is_valid(&json!(12)));
    let mut other = document.clone();
    other["paths"]["/items"] = document["paths"]["/counts"].clone();
    assert!(validator(&other, "xml")?.is_valid(&json!(12)));
    assert!(!validator(&other, "xml")?.is_valid(&json!("text")));
    let decision = report
        .decisions
        .iter()
        .find(|decision| {
            decision.code == "RAML_SCHEMA_FRAGMENT"
                && decision.source.address.uri.ends_with("body.raml")
        })
        .ok_or("physical selection finding")?;
    assert_eq!(
        decision.source.address.pointer,
        "/body/application~1xml/type"
    );
    assert_eq!(decision.source.line, Some(3));
    Ok(())
}

#[test]
fn library_and_template_selection_retains_physical_include_base_and_placement()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request("schema.xsd#Text", XML, true)?;
    input.input.content=Some("#%RAML 1.0\ntitle: Library\nversion: v1\nbaseUri: https://api.example.test\nuses:\n  contracts: lib/types.raml\n/items:\n  post:\n    body:\n      application/xml: contracts.Selected\n    responses:\n      204: {description: Empty}\n".into());
    input.files.insert(
        "lib/types.raml".into(),
        "#%RAML 1.0 Library\ntypes:\n  Selected: !include ../traits/schema.xsd#Text\n".into(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("output")?;
    assert!(validator(&document, "xml")?.is_valid(&json!("ok")));
    assert!(!validator(&document, "xml")?.is_valid(&json!(12)));
    input.input.content = input
        .input
        .content
        .map(|source| source.replace("application/xml", "application/json"));
    assert_eq!(
        convert(&input).err().ok_or("invalid media accepted")?.code,
        "RAML_SCHEMA_PLACEMENT"
    );
    Ok(())
}

#[test]
fn invalid_schema_selectors_never_silently_select_whole_document()
-> Result<(), Box<dyn std::error::Error>> {
    for selector in ["Missing", "Text/child", "Text%XX", "%FF"] {
        assert_eq!(
            convert(&request(&format!("schema.xsd#{selector}"), XML, true)?)
                .err()
                .ok_or("invalid XML selector accepted")?
                .code,
            "RAML_SCHEMA_FRAGMENT"
        );
    }
    for selector in [
        "Anchor",
        "/missing",
        "/properties/value/type",
        "/bad~2",
        "/%FF",
        "/%XX",
    ] {
        let schema = r#"{"properties":{"value":{"type":"string"}}}"#;
        assert_eq!(
            convert(&request(&format!("schema.json#{selector}"), schema, false)?)
                .err()
                .ok_or("invalid JSON selector accepted")?
                .code,
            "RAML_SCHEMA_FRAGMENT"
        );
    }
    let ambiguous = XML.replace(
        "<x:element name=\"Text\"",
        "<x:complexType name=\"Text\"/><x:element name=\"Text\"",
    );
    assert_eq!(
        convert(&request("schema.xsd#Text", &ambiguous, true)?)
            .err()
            .ok_or("ambiguous selector accepted")?
            .code,
        "RAML_SCHEMA_FRAGMENT"
    );
    Ok(())
}

#[test]
fn selected_schema_can_live_in_ignored_reference_siblings_or_nested_dialects()
-> Result<(), Box<dyn std::error::Error>> {
    let schema =
        json!({"$schema":"http://json-schema.org/draft-07/schema#","$ref":"#/definitions/Other",
        "definitions":{"Payload":{"type":"integer","minimum":2},"Other":{"type":"string"}}})
        .to_string();
    let report = convert(&request(
        "schema.json#/definitions/Payload",
        &schema,
        false,
    )?)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let check = validator(&report.document.ok_or("output")?, "json")?;
    assert!(check.is_valid(&json!(2)));
    assert!(!check.is_valid(&json!(1)));
    assert!(!check.is_valid(&json!("wrong")));
    let schema=json!({"$schema":"https://json-schema.org/draft/2020-12/schema","$defs":{
        "Legacy":{"$schema":"http://json-schema.org/draft-07/schema#","$id":"https://schemas.test/legacy","definitions":{
            "Payload":{"type":"array","items":[{"type":"integer"}],"additionalItems":false}
        }}
    }}).to_string();
    let report = convert(&request(
        "schema.json#/$defs/Legacy/definitions/Payload",
        &schema,
        false,
    )?)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let check = validator(&report.document.ok_or("output")?, "json")?;
    assert!(check.is_valid(&json!([2])));
    assert!(!check.is_valid(&json!(["wrong"])));
    assert!(!check.is_valid(&json!([2, 3])));
    Ok(())
}

#[test]
fn complete_enclosing_resource_keeps_reference_validation_and_all_targets()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = r#"{"$defs":{"Payload":{"type":"string"},"Other":{"$ref":"missing.json"}}}"#;
    let error = convert(&request("schema.json#/$defs/Payload", schema, false)?)
        .err()
        .ok_or("unresolved enclosing reference accepted")?;
    assert_eq!(error.code, "SOURCE_INVALID");
    for target in octri_converter::TARGET_VERSIONS {
        for (path, source, xml) in [
            (
                "schema.json#/$defs/Payload",
                r#"{"$defs":{"Payload":{"type":"string"}}}"#,
                false,
            ),
            ("schema.xsd#Text", XML, true),
        ] {
            let mut input = request(path, source, xml)?;
            input.config.target_version = Some((*target).into());
            let report = convert(&input)?;
            assert!(report.valid, "{target}: {:?}", report.diagnostics);
            assert_ne!(
                report.status,
                octri_converter::Status::Blocked,
                "{target}: {:?}",
                report.decisions
            );
        }
    }
    Ok(())
}

#[test]
fn selected_template_schema_uses_rendered_json_and_xml_resources()
-> Result<(), Box<dyn std::error::Error>> {
    for (media, path, schema) in [
        (
            "json",
            "schema.json#/$defs/Body",
            r#"{"$defs":{"Body":{"const":"<<value>>"}}}"#,
        ),
        (
            "xml",
            "schema.xsd#Body",
            r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema"><x:element name="Body"><x:simpleType><x:restriction base="x:string"><x:enumeration value="<<value>>"/></x:restriction></x:simpleType></x:element></x:schema>"#,
        ),
    ] {
        let mut input = request(path, schema, media == "xml")?;
        input.input.content = input
            .input
            .content
            .map(|text| text.replace("is: [payload]", "is: [{payload: {value: done}}]"));
        input.input.content.as_mut().ok_or("root")?.push_str("/other:\n  post:\n    is: [{payload: {value: changed}}]\n    responses:\n      204: {description: Empty}\n");
        let report = convert(&input)?;
        assert!(report.valid, "{media}: {:?}", report.diagnostics);
        let mut document = report.document.ok_or("output")?;
        let check = validator(&document, media)?;
        assert!(check.is_valid(&json!("done")), "{media}");
        assert!(!check.is_valid(&json!("<<value>>")), "{media}");
        assert!(!check.is_valid(&json!("other")), "{media}");
        document["paths"]["/items"] = document["paths"]["/other"].clone();
        assert!(validator(&document, media)?.is_valid(&json!("changed")));
        assert!(!validator(&document, media)?.is_valid(&json!("done")));
    }
    Ok(())
}
