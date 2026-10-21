//! RAML XSD bodies reuse QName/import projection without losing source evidence.
use octri_converter::{ConversionRequest, Status, convert};
use serde_json::{Value, json};

const SCHEMA: &str = r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema" xmlns:c="urn:counts" targetNamespace="urn:items" elementFormDefault="qualified">
  <x:import namespace="urn:counts" schemaLocation="count.xsd"/>
  <x:element name="Item"><x:complexType><x:all>
    <x:element name="count" type="c:Count"/>
    <x:element name="label" type="x:string" minOccurs="0"/>
  </x:all></x:complexType></x:element>
</x:schema>"#;
const COUNT: &str = r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:counts"><x:simpleType name="Count"><x:restriction base="x:integer"><x:minInclusive value="2"/></x:restriction></x:simpleType></x:schema>"#;

fn request(version: &str, schema: &str) -> Result<ConversionRequest, serde_json::Error> {
    let facet = if version == "0.8" { "schema" } else { "type" };
    serde_json::from_value(
        json!({"input":{"uri":"memory://raml/api.raml","content":format!("#%RAML {version}\ntitle: XML bodies\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n{}\n/items:\n  post:\n    is: [payload]\n    responses:\n      204: {{description: Empty}}\n", if version=="0.8" {"  - payload: !include traits/body.raml"} else {"  payload: !include traits/body.raml"})},"files":{
            "traits/body.raml":format!("body:\n  application/xml:\n    {facet}: {}\n",json!(schema).to_string()),
            "traits/count.xsd":COUNT
        }}),
    )
}

fn validator(
    document: &Value,
    media: &str,
) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let registry = jsonschema::Registry::new()
        .add(
            "memory://output/api.json",
            jsonschema::Resource::from_contents(document.clone()),
        )?
        .prepare()?;
    Ok(jsonschema::options().with_registry(&registry).build(&json!({"$ref":format!("memory://output/api.json#/paths/~1items/post/requestBody/content/{}/schema",media.replace('~',"~0").replace('/',"~1"))}))?)
}

#[test]
fn legacy_and_modern_inline_xsd_bodies_keep_imports_constraints_and_xml_names()
-> Result<(), Box<dyn std::error::Error>> {
    for version in ["0.8", "1.0"] {
        let report = convert(&request(version, SCHEMA)?)?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
        assert!(
            report
                .decisions
                .iter()
                .any(|decision| decision.code == "RAML_XML_SERIALIZATION"
                    && decision.review_required)
        );
        assert_eq!(report.manifest.files.len(), 3);
        assert!(!serde_json::to_string(&report.manifest.files)?.contains(".octri-xsd-"));
        let document = report.document.ok_or("output")?;
        let check = validator(&document, "application/xml")?;
        assert!(check.is_valid(&json!({"count":2,"label":"ok"})));
        assert!(!check.is_valid(&json!({"count":1})));
        assert!(!check.is_valid(&json!({"label":"missing"})));
        assert!(!check.is_valid(&json!({"count":"wrong"})));
        assert!(
            document["components"]["schemas"]
                .as_object()
                .ok_or("schemas")?
                .values()
                .any(|schema| schema.pointer("/xml/name") == Some(&json!("Item"))
                    && schema.pointer("/xml/namespace") == Some(&json!("urn:items")))
        );
    }
    Ok(())
}

#[test]
fn included_xsd_and_chameleon_includes_keep_physical_xml_locations()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request("1.0", SCHEMA)?;
    input.files.insert(
        "traits/body.raml".into(),
        "body:\n  application/xml:\n    type: !include schemas/input.xsd\n".into(),
    );
    input.files.insert("traits/schemas/input.xsd".into(),r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:items"><x:include schemaLocation="item.xsd"/></x:schema>"#.into());
    input.files.insert("traits/schemas/item.xsd".into(),r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema"><x:element name="Item" type="x:string" nillable="true"/></x:schema>"#.into());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let decision = report
        .decisions
        .iter()
        .find(|decision| decision.code == "XSD_NIL_SERIALIZATION")
        .ok_or("nil finding")?;
    assert_eq!(
        decision.source.address.uri,
        "memory://raml/traits/schemas/item.xsd"
    );
    assert!(decision.source.address.pointer.starts_with("/xml/"));
    assert_eq!(decision.source.line, Some(1));
    let document = report.document.ok_or("output")?;
    let check = validator(&document, "application/xml")?;
    assert!(check.is_valid(&json!("value")));
    assert!(check.is_valid(&json!(null)));
    assert!(!check.is_valid(&json!(123)));
    Ok(())
}

#[test]
fn inline_xml_findings_and_syntax_errors_point_to_authored_scalar()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema"><x:element name="Item" type="x:string" nillable="true"/></x:schema>"#;
    let report = convert(&request("1.0", schema)?)?;
    let decision = report
        .decisions
        .iter()
        .find(|decision| decision.code == "XSD_NIL_SERIALIZATION")
        .ok_or("nil finding")?;
    assert_eq!(
        decision.source.address.uri,
        "memory://raml/traits/body.raml"
    );
    assert_eq!(
        decision.source.address.pointer,
        "/body/application~1xml/type"
    );
    assert_eq!(decision.source.line, Some(3));
    let error = convert(&request("1.0", "<broken>")?)
        .err()
        .ok_or("invalid XML accepted")?;
    assert!(!error.diagnostics.is_empty());
    assert_eq!(
        error.diagnostics[0].location.address.pointer,
        "/body/application~1xml/type"
    );
    assert_eq!(error.diagnostics[0].location.line, Some(3));
    Ok(())
}

#[test]
fn named_xml_aliases_keep_media_and_raml_placement_restrictions()
-> Result<(), Box<dyn std::error::Error>> {
    let schema=json!(r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema"><x:element name="Item" type="x:string"/></x:schema>"#).to_string();
    for facet in ["type", "schema"] {
        let prefix = format!(
            "#%RAML 1.0\ntitle: XML alias\nversion: v1\nbaseUri: https://api.example.test\ntypes:\n  Raw: {schema}\n  Alias: ' Raw '\n"
        );
        for (declaration, expected) in [
            (
                format!(
                    "/items:\n  post:\n    body:\n      application/problem+xml:\n        {facet}: Alias\n    responses:\n      204: {{description: Empty}}\n"
                ),
                None,
            ),
            (
                format!(
                    "/items:\n  post:\n    body:\n      application/json:\n        {facet}: Alias\n"
                ),
                Some("RAML_SCHEMA_PLACEMENT"),
            ),
            (
                format!(
                    "/items:\n  get:\n    queryParameters:\n      q:\n        {facet}: Alias\n"
                ),
                Some("RAML_SCHEMA_PLACEMENT"),
            ),
            (
                format!(
                    "/items:\n  post:\n    body:\n      application/xml:\n        {facet}: Alias\n        minLength: 2\n"
                ),
                Some("RAML_SCHEMA_SPECIALIZATION"),
            ),
        ] {
            let mut input = request("1.0", "<unused/>")?;
            input.input.content = Some(format!("{prefix}{declaration}"));
            input.files.clear();
            if let Some(expected) = expected {
                assert_eq!(
                    convert(&input)
                        .err()
                        .ok_or("invalid schema use accepted")?
                        .code,
                    expected
                );
            } else {
                let report = convert(&input)?;
                assert!(report.valid, "{:?}", report.diagnostics);
                let document = report.document.ok_or("output")?;
                assert!(validator(&document, "application/problem+xml")?.is_valid(&json!("ok")));
            }
        }
    }
    Ok(())
}

#[test]
fn distinct_same_qname_schemas_do_not_share_component_constraints()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(
        "1.0",
        r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:items"><x:element name="Item" type="x:string"/></x:schema>"#,
    )?;
    let second=json!(r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:items"><x:element name="Item" type="x:boolean"/></x:schema>"#).to_string();
    input.input.content.as_mut().ok_or("root")?.push_str(&format!("/other:\n  post:\n    body:\n      application/xml:\n        type: {second}\n    responses:\n      204: {{description: Empty}}\n"));
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("output")?;
    let first = validator(&document, "application/xml")?;
    assert!(first.is_valid(&json!("string")));
    assert!(!first.is_valid(&json!(true)));
    let mut other = document.clone();
    other["paths"]["/items"] = document["paths"]["/other"].clone();
    let second = validator(&other, "application/xml")?;
    assert!(second.is_valid(&json!(true)));
    assert!(!second.is_valid(&json!("string")));
    Ok(())
}

#[test]
fn xml_imports_and_entities_never_gain_ambient_io() -> Result<(), Box<dyn std::error::Error>> {
    let mut missing = request("1.0", SCHEMA)?;
    missing.files.remove("traits/count.xsd");
    assert_eq!(
        convert(&missing)
            .err()
            .ok_or("missing import accepted")?
            .code,
        "SOURCE_NOT_SUPPLIED"
    );
    let entity = r#"<!DOCTYPE schema SYSTEM "https://attacker.test/schema.dtd"><schema/>"#;
    assert_eq!(
        convert(&request("1.0", entity)?)
            .err()
            .ok_or("DOCTYPE accepted")?
            .code,
        "XML_DTD_DISABLED"
    );
    let mut limited = request("1.0", SCHEMA)?;
    limited.config.limits.max_nodes = 30;
    assert!(convert(&limited).is_err());
    Ok(())
}

#[test]
fn xml_body_projection_exports_every_published_target() -> Result<(), Box<dyn std::error::Error>> {
    for target in octri_converter::TARGET_VERSIONS {
        let mut input = request("1.0", SCHEMA)?;
        input.config.target_version = Some((*target).into());
        let report = convert(&input)?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_ne!(
            report.status,
            Status::Blocked,
            "{target}: {:?}",
            report.decisions
        );
        let document = report.document.ok_or("output")?;
        assert_eq!(
            document.get("openapi").or_else(|| document.get("swagger")),
            Some(&json!(target))
        );
    }
    Ok(())
}

#[test]
fn xml_bom_headers_keep_inline_include_and_library_contracts()
-> Result<(), Box<dyn std::error::Error>> {
    for version in ["0.8", "1.0"] {
        for included in [false, true] {
            let mut input = request(version, &format!("\u{feff}{SCHEMA}"))?;
            if included {
                let facet = if version == "0.8" { "schema" } else { "type" };
                input.files.insert(
                    "traits/body.raml".into(),
                    format!("body:\n  application/xml:\n    {facet}: !include input.xsd\n"),
                );
                input
                    .files
                    .insert("traits/input.xsd".into(), format!("\u{feff}{SCHEMA}"));
            }
            let report = convert(&input)?;
            assert!(report.valid, "{:?}", report.diagnostics);
            let document = report.document.ok_or("output")?;
            assert!(validator(&document, "application/xml")?.is_valid(&json!({"count":2})));
            assert!(!validator(&document, "application/xml")?.is_valid(&json!({"count":1})));
        }
    }
    let mut input = request("1.0", "<unused/>")?;
    let schema=json!("\u{feff}<x:schema xmlns:x=\"http://www.w3.org/2001/XMLSchema\"><x:element name=\"Code\" type=\"x:string\"/></x:schema>").to_string();
    input.input.content=Some("#%RAML 1.0\ntitle: Library BOM\nversion: v1\nbaseUri: https://api.example.test\nuses:\n  contracts: lib/contracts.raml\n/items:\n  post:\n    body:\n      application/xml: contracts.Code\n    responses:\n      204: {description: Empty}\n".into());
    input.files.clear();
    input.files.insert(
        "lib/contracts.raml".into(),
        format!("#%RAML 1.0 Library\ntypes:\n  Code: {schema}\n"),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(
        report.document.ok_or("output")?["components"]["schemas"]
            .as_object()
            .ok_or("schemas")?
            .values()
            .any(|schema| schema.pointer("/xml/name") == Some(&json!("Code")))
    );
    Ok(())
}

fn mixed_request(
    json_schema: Value,
    xml_schema: &str,
    aliases: bool,
) -> Result<ConversionRequest, serde_json::Error> {
    let extra = if aliases {
        "  JsonB: *json\n  XmlB: *xml\n"
    } else {
        ""
    };
    serde_json::from_value(
        json!({"input":{"uri":"memory://mixed/api.raml","content":format!(
        "#%RAML 1.0\ntitle: Mixed\nversion: v1\nbaseUri: https://api.example.test\ntypes:\n  Json: &json {}\n  Xml: &xml {}\n{extra}/items:\n  post:\n    body:\n      application/xml: Xml\n    responses:\n      204: {{description: Empty}}\n",
        json!(json_schema.to_string()),json!(xml_schema))}}),
    )
}

#[test]
fn json_and_xml_derived_resources_share_document_byte_and_node_limits()
-> Result<(), Box<dyn std::error::Error>> {
    let xml = r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema"><x:element name="Item" type="x:string"/></x:schema>"#;
    let mut input = mixed_request(json!({"type":"string"}), xml, false)?;
    input.config.limits.max_documents = 1;
    assert_eq!(
        convert(&input)
            .err()
            .ok_or("mixed document limit bypassed")?
            .code,
        "SCHEMA_DERIVED_LIMIT"
    );
    input.config.limits.max_documents = 2;
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(
        validator(&report.document.ok_or("output")?, "application/xml")?.is_valid(&json!("ok"))
    );

    let description = "x".repeat(1000);
    let xml = format!(
        r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema"><x:annotation><x:documentation>{description}</x:documentation></x:annotation><x:element name="Item" type="x:string"/></x:schema>"#
    );
    let mut input = mixed_request(
        json!({"type":"string","description":description}),
        &xml,
        true,
    )?;
    let original_bytes = input.input.content.as_ref().ok_or("source")?.len();
    input.config.limits.max_total_bytes = original_bytes + 32;
    assert_eq!(
        convert(&input)
            .err()
            .ok_or("mixed byte limit bypassed")?
            .code,
        "SOURCE_INVALID"
    );
    input.config.limits.max_total_bytes = 20 * 1024 * 1024;
    assert!(convert(&input)?.valid);

    let mut input = mixed_request(
        json!({"type":"integer","enum":(0..100).collect::<Vec<_>>()}),
        xml.as_str(),
        false,
    )?;
    input.config.limits.max_nodes = 105;
    assert_eq!(
        convert(&input)
            .err()
            .ok_or("mixed node limit bypassed")?
            .code,
        "SCHEMA_DERIVED_LIMIT"
    );
    Ok(())
}
