use octri_converter::{Config, ConversionRequest, convert};
use octri_parser::Input;
use serde_json::json;

#[test]
fn prohibited_xml_particle_means_absence_and_invalid_occurrence_bounds_fail()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"<w:definitions xmlns:w="http://schemas.xmlsoap.org/wsdl/" xmlns:x="http://www.w3.org/2001/XMLSchema" xmlns:t="urn:types" targetNamespace="urn:types">
      <w:types><x:schema targetNamespace="urn:types"><x:complexType name="Input"><x:all>
        <x:element name="retired" type="x:string" minOccurs="0" maxOccurs="0"/>
      </x:all></x:complexType></x:schema></w:types>
      <w:message name="Input"><w:part name="body" type="t:Input"/></w:message>
      <w:portType name="Service"><w:operation name="Create"><w:input message="t:Input"/></w:operation></w:portType>
    </w:definitions>"#;
    let mut input: ConversionRequest = serde_json::from_value(
        json!({"input":{"uri":"memory://xsd/service.wsdl","content":source},"config":{"apiVersion":"1","httpMappings":{"{urn:types}Service.Create":{"method":"POST","path":"/input","server":"https://api.example.com","responseStatus":"204","security":[]}}}}),
    )?;
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("missing output")?;
    let schemas = document["components"]["schemas"]
        .as_object()
        .ok_or("missing schemas")?;
    let schema = schemas
        .values()
        .find(|schema| schema.pointer("/properties/retired").is_some())
        .ok_or("missing projected particle")?;
    assert_eq!(schema["properties"]["retired"], json!(false));
    let validator = jsonschema::validator_for(schema)?;
    assert!(validator.is_valid(&json!({})));
    assert!(!validator.is_valid(&json!({"retired":[]})));
    input.input.content = Some(source.replace("minOccurs=\"0\"", "minOccurs=\"1\""));
    assert_eq!(
        convert(&input)
            .err()
            .ok_or("invalid occurrence accepted")?
            .code,
        "XSD_OCCURRENCE"
    );
    Ok(())
}

#[test]
fn inheritance_accepts_derived_fields_and_keeps_base_requirements()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"<w:definitions xmlns:w="http://schemas.xmlsoap.org/wsdl/" xmlns:x="http://www.w3.org/2001/XMLSchema" xmlns:t="urn:types" targetNamespace="urn:types">
      <w:types><x:schema targetNamespace="urn:types">
        <x:complexType name="Base"><x:all><x:element name="id" type="x:integer"/></x:all></x:complexType>
        <x:complexType name="Derived"><x:complexContent><x:extension base="t:Base"><x:all><x:element name="name" type="x:string"/></x:all></x:extension></x:complexContent></x:complexType>
      </x:schema></w:types>
      <w:message name="Input"><w:part name="body" type="t:Derived"/></w:message>
      <w:portType name="Service"><w:operation name="Create"><w:input message="t:Input"/></w:operation></w:portType>
    </w:definitions>"#;
    let mut input = ConversionRequest {
        input: Input {
            uri: "memory://xsd/service.wsdl".into(),
            content: Some(source.into()),
            generated: false,
        },
        files: Default::default(),
        config: Config {
            api_version: Some("1".into()),
            ..Default::default()
        },
    };
    input.config.http_mappings.insert("{urn:types}Service.Create".into(),serde_json::from_value(json!({"method":"POST","path":"/derived","server":"https://api.example.com","responseStatus":"204","security":[]}))?);
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("missing output")?;
    let names = document
        .pointer("/components/schemas")
        .and_then(serde_json::Value::as_object)
        .ok_or("missing schema registry")?;
    let derived = names
        .keys()
        .find(|name| name.starts_with("XsdType_Derived_"))
        .ok_or("missing derived schema")?;
    let registry = jsonschema::Registry::new()
        .add(
            "memory://projected/api.json",
            jsonschema::Resource::from_contents(document.clone()),
        )?
        .prepare()?;
    let entry =
        json!({"$ref":format!("memory://projected/api.json#/components/schemas/{derived}")});
    let validator = jsonschema::options()
        .with_registry(&registry)
        .build(&entry)?;
    assert!(validator.is_valid(&json!({"id":1,"name":"Spot"})));
    assert!(!validator.is_valid(&json!({"name":"Spot"})));
    assert!(!validator.is_valid(&json!({"id":1,"name":"Spot","unexpected":true})));
    Ok(())
}
