use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::{Value, json};

fn request(types: &str) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://serialization/api.raml".into(),
            content: Some(format!(
                "#%RAML 1.0\ntitle: Serialization\nversion: v1\nbaseUri: https://api.example.test\ntypes:\n{types}\n/items:\n  get:\n    responses:\n      204: {{description: Empty}}\n"
            )),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    }
}

fn schemas(document: &Value) -> &Value {
    if document.get("swagger").is_some() {
        &document["definitions"]
    } else {
        &document["components"]["schemas"]
    }
}

#[test]
fn xml_facets_preserve_names_attributes_and_inherited_arrays_for_all_targets()
-> Result<(), Box<dyn std::error::Error>> {
    let declarations = "  Identifier: string\n  Labels: string[]\n  Person:\n    type: object\n    xml: {name: person, namespace: 'https://example.test/people', prefix: p}\n    properties:\n      id:\n        type: Identifier\n        xml: {attribute: true, name: personId}\n      labels:\n        type: Labels\n        xml: {wrapped: true, name: labels}\n      enabled:\n        type: boolean\n        xml: {attribute: false}\n    example:\n      id: '1'\n      labels: [one]\n      enabled: true\n";
    for target in octri_converter::TARGET_VERSIONS {
        let mut input = request(declarations);
        input.config.target_version = Some((*target).into());
        let report = convert(&input)?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_eq!(
            report.status,
            Status::Complete,
            "{target}: {:?}",
            report.decisions
        );
        let document = report.document.ok_or("output")?;
        let person = &schemas(&document)["Person"];
        assert_eq!(
            person["xml"],
            json!({"name":"person","namespace":"https://example.test/people","prefix":"p"})
        );
        assert_eq!(
            person["properties"]["id"]["xml"],
            json!({"attribute":true,"name":"personId"})
        );
        assert_eq!(person["properties"]["labels"]["type"], "array");
        assert_eq!(
            person["properties"]["labels"]["xml"],
            json!({"wrapped":true,"name":"labels"})
        );
        assert_eq!(
            person["example"],
            json!({"id":"1","labels":["one"],"enabled":true})
        );
        assert!(
            !report
                .decisions
                .iter()
                .any(|decision| decision.code == "RAML_FACET")
        );
    }
    Ok(())
}

#[test]
fn invalid_xml_flags_and_type_combinations_have_physical_diagnostics()
-> Result<(), Box<dyn std::error::Error>> {
    for declaration in [
        "{type: string, xml: null}",
        "{type: string, xml: {name: true}}",
        "{type: string, xml: {wrapped: yes}}",
        "{type: string, xml: {prefix: 1}}",
        "{type: string, xml: {wrapped: true}}",
        "{type: object, xml: {attribute: true}}",
        "{type: array, xml: {attribute: true}}",
        "{type: string, xml: {attribute: true, wrapped: true}}",
        "{type: Base, xml: {attribute: true}}",
    ] {
        let input = request(&format!("  Base: object\n  Bad: {declaration}\n"));
        let error = convert(&input)
            .err()
            .ok_or_else(|| format!("Accepted {declaration}"))?;
        assert_eq!(error.code, "RAML_SERIALIZATION", "{declaration}");
        assert!(
            error.diagnostics[0]
                .location
                .address
                .pointer
                .starts_with("/types/Bad/xml")
        );
        assert!(error.diagnostics[0].location.line.is_some());
    }
    Ok(())
}

#[test]
fn file_facets_keep_byte_limits_separate_from_string_constraints()
-> Result<(), Box<dyn std::error::Error>> {
    let input = request(
        "  Upload:\n    type: file\n    fileTypes: [image/png, 'image/*']\n    minLength: 2\n    maxLength: 8\n  Alias:\n    type: Upload\n    minLength: 4\n  Text:\n    type: string\n    minLength: 2\n",
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview);
    let document = report.document.as_ref().ok_or("output")?;
    let upload = &schemas(document)["Upload"];
    assert_eq!(upload["x-raml-file-types"], json!(["image/png", "image/*"]));
    assert_eq!(
        upload["x-raml-file-byte-length"],
        json!({"minLength":2,"maxLength":8})
    );
    assert!(upload.get("minLength").is_none() && upload.get("maxLength").is_none());
    assert_eq!(
        schemas(document)["Alias"]["x-raml-file-byte-length"],
        json!({"minLength":4})
    );
    assert!(schemas(document)["Alias"].get("minLength").is_none());
    assert_eq!(schemas(document)["Text"]["minLength"], 2);
    assert!(jsonschema::validator_for(upload)?.is_valid(&json!("é")));
    assert!(!jsonschema::validator_for(&schemas(document)["Text"])?.is_valid(&json!("é")));
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "RAML_FILE_TYPES" && decision.review_required)
    );
    let mut reviewed = input;
    reviewed.config.accepted_decisions = report
        .decisions
        .iter()
        .map(|decision| decision.id.clone())
        .collect();
    assert_eq!(convert(&reviewed)?.status, Status::Complete);
    Ok(())
}

#[test]
fn malformed_file_facets_do_not_become_annotations() -> Result<(), Box<dyn std::error::Error>> {
    for declaration in [
        "{type: string, fileTypes: [image/png]}",
        "{type: file, fileTypes: []}",
        "{type: file, fileTypes: image/png}",
        "{type: file, fileTypes: [true]}",
        "{type: file, fileTypes: ['image /png']}",
        "{type: file, fileTypes: ['*/png']}",
        "{type: file, minLength: -1}",
        "{type: file, maxLength: 1.5}",
        "{type: file, minLength: 8, maxLength: 2}",
    ] {
        let error = convert(&request(&format!("  Bad: {declaration}\n")))
            .err()
            .ok_or_else(|| format!("Accepted {declaration}"))?;
        assert_eq!(error.code, "RAML_SERIALIZATION", "{declaration}");
    }
    Ok(())
}

#[test]
fn xml_metadata_without_equivalent_fields_requires_review() -> Result<(), Box<dyn std::error::Error>>
{
    let input = request(
        "  Item:\n    type: object\n    xml: {wrapped: true, namespace: relative/path, custom: retained}\n    properties:\n      name: string\n",
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview);
    let document = report.document.ok_or("output")?;
    assert_eq!(
        schemas(&document)["Item"]["x-raml-xml"],
        json!({"wrapped":true,"namespace":"relative/path","custom":"retained"})
    );
    assert!(schemas(&document)["Item"].get("xml").is_none());
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "RAML_XML_SERIALIZATION"
                && decision.review_required
                && decision.source.address.pointer == "/types/Item/xml")
    );
    Ok(())
}

#[test]
fn mixed_unions_are_never_narrowed_by_xml_wrapping() -> Result<(), Box<dyn std::error::Error>> {
    for expression in [
        "object | string[]",
        "string[] | object",
        "(object | string[]) | number[]",
    ] {
        let report = convert(&request(&format!(
            "  Mixed:\n    type: {expression}\n    xml: {{wrapped: true}}\n"
        )))?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_eq!(report.status, Status::NeedsReview);
        let document = report.document.ok_or("output")?;
        let mixed = &schemas(&document)["Mixed"];
        assert!(mixed.get("type").is_none(), "{expression}");
        let check = jsonschema::validator_for(mixed)?;
        assert!(check.is_valid(&json!({"name":"object"})), "{expression}");
        assert!(check.is_valid(&json!(["array"])), "{expression}");
        assert!(!check.is_valid(&json!(true)), "{expression}");
    }
    Ok(())
}

#[test]
fn file_byte_limits_keep_large_exact_integers() -> Result<(), Box<dyn std::error::Error>> {
    let report = convert(&request(
        "  Upload:\n    type: file\n    minLength: 1e2\n    maxLength: 18446744073709551616\n",
    ))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("output")?;
    let limits = &schemas(&document)["Upload"]["x-raml-file-byte-length"];
    assert!(octri_parser::numbers::compare_text(&limits["minLength"].to_string(), "100").is_eq());
    assert_eq!(limits["maxLength"].to_string(), "18446744073709551616");
    Ok(())
}

#[test]
fn file_serialization_metadata_survives_every_target() -> Result<(), Box<dyn std::error::Error>> {
    for target in octri_converter::TARGET_VERSIONS {
        let mut input = request(
            "  Upload:\n    type: file\n    fileTypes: ['*/*']\n    minLength: 2\n    maxLength: 8\n",
        );
        input.config.target_version = Some((*target).into());
        let report = convert(&input)?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_eq!(
            report.status,
            Status::NeedsReview,
            "{target}: {:?}",
            report.decisions
        );
        let document = report.document.ok_or("output")?;
        assert_eq!(
            schemas(&document)["Upload"]["x-raml-file-types"],
            json!(["*/*"])
        );
        assert_eq!(
            schemas(&document)["Upload"]["x-raml-file-byte-length"],
            json!({"minLength":2,"maxLength":8})
        );
    }
    Ok(())
}

#[test]
fn included_serialization_errors_keep_original_locations() -> Result<(), Box<dyn std::error::Error>>
{
    let mut input = request("  Item: !include types/item.raml\n");
    input.files.insert(
        "types/item.raml".into(),
        "#%RAML 1.0 DataType\ntype: object\nxml:\n  attribute: true\n".into(),
    );
    let error = convert(&input)
        .err()
        .ok_or("invalid included XML accepted")?;
    assert_eq!(error.code, "RAML_SERIALIZATION");
    assert_eq!(
        error.diagnostics[0].location.address.uri,
        "memory://serialization/types/item.raml"
    );
    assert_eq!(error.diagnostics[0].location.address.pointer, "/xml");
    assert!(error.diagnostics[0].location.line.is_some());
    Ok(())
}
