use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::json;

fn request(source: &str) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://wsdl/service.wsdl".into(),
            content: Some(source.into()),
            generated: false,
        },
        files: Default::default(),
        config: Config {
            api_version: Some("1".into()),
            ..Default::default()
        },
    }
}

const SERVICE: &str = r#"<?xml version="1.0"?>
<w:definitions xmlns:w="http://schemas.xmlsoap.org/wsdl/" xmlns:x="http://www.w3.org/2001/XMLSchema" xmlns:t="urn:service" xmlns:m="urn:models" targetNamespace="urn:service" name="Pets">
  <w:types><x:schema targetNamespace="urn:models" elementFormDefault="qualified">
    <x:include schemaLocation="models.xsd"/>
    <x:element name="FindPet" type="m:PetId"/>
    <x:element name="Pet"><x:complexType><x:all><x:element name="id" type="m:PetId"/><x:element name="parent" type="m:PetType" minOccurs="0"/></x:all><x:attribute name="kind" type="x:string" use="required"/></x:complexType></x:element>
  </x:schema></w:types>
  <w:message name="FindInput"><w:part name="body" element="m:FindPet"/></w:message>
  <w:message name="FindOutput"><w:part name="body" element="m:Pet"/></w:message>
  <w:portType name="Pets"><w:operation name="Find"><w:input message="t:FindInput"/><w:output message="t:FindOutput"/></w:operation></w:portType>
</w:definitions>"#;

#[test]
fn wsdl_virtual_xsd_imports_qnames_and_recursive_types_are_portable()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(SERVICE);
    input.files.insert("models.xsd".into(),r#"<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema" xmlns:m="urn:models" targetNamespace="urn:models"><x:simpleType name="PetId"><x:restriction base="x:unsignedLong"><x:minInclusive value="1"/></x:restriction></x:simpleType><x:complexType name="PetType"><x:all><x:element name="parent" type="m:PetType" minOccurs="0"/></x:all></x:complexType></x:schema>"#.into());
    let blocked = convert(&input)?;
    assert_eq!(blocked.status, Status::Blocked);
    assert!(
        blocked
            .decisions
            .iter()
            .any(|decision| decision.code == "HTTP_MAPPING_REQUIRED")
    );
    input.config.http_mappings.insert("{urn:service}Pets.Find".into(),serde_json::from_value(json!({"method":"POST","path":"/pets/find","server":"https://api.example.com","responseStatus":"200","security":[]}))?);
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    assert!(
        report
            .document
            .as_ref()
            .and_then(|doc| doc.pointer(
                "/paths/~1pets~1find/post/responses/200/content/application~1json/schema/$ref"
            ))
            .is_some()
    );
    assert!(
        report
            .document
            .as_ref()
            .is_some_and(|doc| doc.to_string().contains("18446744073709551615"))
    );
    let document = report.document.ok_or("missing output")?;
    let parsed = octri_parser::execute(
        octri_parser::Request {
            operation: octri_parser::Operation::Analyze,
            input: Input {
                uri: "memory://relocated/openapi.json".into(),
                content: Some(document.to_string()),
                generated: true,
            },
            options: Default::default(),
        },
        &mut octri_parser::NoIo,
    )?;
    assert!(parsed.valid, "{:?}", parsed.diagnostics);
    Ok(())
}

#[test]
fn wsdl_imports_never_read_ambient_files_or_accept_namespace_spoofing()
-> Result<(), Box<dyn std::error::Error>> {
    assert!(convert(&request(SERVICE)).is_err());
    let mut input = request(SERVICE);
    input.files.insert(
        "models.xsd".into(),
        "<schema xmlns='http://www.w3.org/2001/XMLSchema' targetNamespace='urn:different'/>".into(),
    );
    assert_eq!(
        convert(&input)
            .err()
            .ok_or("accepted wrong namespace")?
            .code,
        "XSD_IMPORT_NAMESPACE"
    );
    let spoofed = "<definitions xmlns='urn:fake' name='NotWsdl'/>";
    assert!(convert(&request(spoofed)).is_err());
    let entity = "<!DOCTYPE definitions SYSTEM 'file:///etc/passwd'><definitions xmlns='http://schemas.xmlsoap.org/wsdl/'/>";
    assert_eq!(
        convert(&request(entity)).err().ok_or("accepted DTD")?.code,
        "XML_DTD_DISABLED"
    );
    Ok(())
}

#[test]
fn wsdl20_message_elements_need_explicit_endpoint_mappings()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"<description xmlns="http://www.w3.org/ns/wsdl" xmlns:t="urn:example" xmlns:x="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:example"><types><x:schema targetNamespace="urn:example"><x:element name="Ping" type="x:string"/></x:schema></types><interface name="Service"><operation name="Ping" pattern="http://www.w3.org/ns/wsdl/in-out"><input element="t:Ping"/><output element="t:Ping"/></operation></interface></description>"#;
    let mut input = request(source);
    input.config.http_mappings.insert("{urn:example}Service.Ping".into(),serde_json::from_value(json!({"method":"POST","path":"/ping","server":"https://api.example.com","responseStatus":"200","security":[]}))?);
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    Ok(())
}

fn soap_service(namespace: &str, bindings: &str, operations: &str) -> String {
    format!(
        r#"<w:definitions xmlns:w="http://schemas.xmlsoap.org/wsdl/" xmlns:x="http://www.w3.org/2001/XMLSchema" xmlns:t="urn:soap" xmlns:s="{namespace}" targetNamespace="urn:soap" name="Echo">
      <w:types><x:schema targetNamespace="urn:soap"><x:element name="Text" type="x:string"/></x:schema></w:types>
      <w:message name="Text"><w:part name="body" element="t:Text"/></w:message>
      <w:portType name="Echo">{operations}</w:portType>
      <w:binding name="Binding" type="t:Echo"><s:binding style="document" transport="http://schemas.xmlsoap.org/soap/http"/>{bindings}</w:binding>
      <w:service name="Echo"><w:port name="Main" binding="t:Binding"><s:address location="https://api.example.com/soap"/></w:port></w:service>
    </w:definitions>"#
    )
}

#[test]
fn soap_empty_body_retains_envelope_and_declared_header() -> Result<(), Box<dyn std::error::Error>>
{
    let source = soap_service(
        "http://schemas.xmlsoap.org/wsdl/soap/",
        r#"<w:operation name="Ping"><s:operation soapAction="urn:ping"/><w:input><s:body use="literal" parts=""/><s:header message="t:Text" part="body" use="literal"/></w:input></w:operation>"#,
        r#"<w:operation name="Ping"><w:input message="t:Text"/></w:operation>"#,
    );
    let report = convert(&request(&source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("missing output")?;
    let schema = document
        .pointer("/paths/~1soap/post/requestBody/content/text~1xml/schema")
        .ok_or("empty SOAP message lost its envelope")?;
    assert_eq!(schema["properties"]["Body"]["maxProperties"], 0);
    assert!(
        schema
            .pointer("/properties/Header/properties/body")
            .is_some()
    );
    Ok(())
}

#[test]
fn soap11_native_binding_keeps_action_envelope_headers_faults_and_shared_variants()
-> Result<(), Box<dyn std::error::Error>> {
    let operations = r#"<w:operation name="Echo"><w:input message="t:Text"/><w:output message="t:Text"/><w:fault name="Failure" message="t:Text"/></w:operation><w:operation name="EchoAgain"><w:input message="t:Text"/><w:output message="t:Text"/></w:operation>"#;
    let bindings = r#"<w:operation name="Echo"><s:operation soapAction="urn:echo"/><w:input><s:body use="literal"/><s:header message="t:Text" part="body" use="literal"/></w:input><w:output><s:body use="literal"/></w:output></w:operation><w:operation name="EchoAgain"><s:operation soapAction="urn:echo-again"/><w:input><s:body use="literal"/></w:input><w:output><s:body use="literal"/></w:output></w:operation>"#;
    let report = convert(&request(&soap_service(
        "http://schemas.xmlsoap.org/wsdl/soap/",
        bindings,
        operations,
    )))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview);
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "WSDL_SHARED_HTTP_ENDPOINT")
    );
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "SOAP_HEADER_PROCESSING")
    );
    let document = report.document.ok_or("missing output")?;
    let operation = document
        .pointer("/paths/~1soap/post")
        .ok_or("missing native operation")?;
    assert_eq!(operation["servers"][0]["url"], "https://api.example.com");
    assert_eq!(
        operation["x-wsdl-variants"].as_array().map(Vec::len),
        Some(2)
    );
    assert!(
        operation
            .pointer("/requestBody/content/text~1xml/schema/anyOf/0/properties/Header")
            .is_some()
    );
    assert_eq!(operation["parameters"][0]["name"], "SOAPAction");
    assert!(
        operation
            .pointer("/responses/500/content/text~1xml/schema/anyOf")
            .is_some()
    );
    assert!(
        operation
            .pointer("/responses/2XX/content/text~1xml/schema/anyOf")
            .is_some()
    );
    Ok(())
}

#[test]
fn soap12_uses_soap_xml_media_and_explicit_action_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let operations = r#"<w:operation name="Echo"><w:input message="t:Text"/><w:output message="t:Text"/></w:operation>"#;
    let bindings = r#"<w:operation name="Echo"><s:operation soapAction="urn:echo"/><w:input><s:body use="literal"/></w:input><w:output><s:body use="literal"/></w:output></w:operation>"#;
    let report = convert(&request(&soap_service(
        "http://schemas.xmlsoap.org/wsdl/soap12/",
        bindings,
        operations,
    )))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("missing output")?;
    assert_eq!(
        document.pointer("/paths/~1soap/post/x-soap-action"),
        Some(&json!("urn:echo"))
    );
    assert_eq!(
        document.pointer(
            "/paths/~1soap/post/requestBody/content/application~1soap+xml/schema/xml/namespace"
        ),
        Some(&json!("http://www.w3.org/2003/05/soap-envelope"))
    );
    Ok(())
}

#[test]
fn wsdl11_http_binding_uses_declared_path_and_query_parts() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"<w:definitions xmlns:w="http://schemas.xmlsoap.org/wsdl/" xmlns:x="http://www.w3.org/2001/XMLSchema" xmlns:t="urn:http" xmlns:h="http://schemas.xmlsoap.org/wsdl/http/" xmlns:m="http://schemas.xmlsoap.org/wsdl/mime/" targetNamespace="urn:http">
      <w:message name="Input"><w:part name="id" type="x:string"/><w:part name="limit" type="x:int"/></w:message><w:message name="Output"><w:part name="result" type="x:string"/></w:message>
      <w:portType name="Pets"><w:operation name="Find"><w:input message="t:Input"/><w:output message="t:Output"/></w:operation></w:portType>
      <w:binding name="Binding" type="t:Pets"><h:binding verb="GET"/><w:operation name="Find"><h:operation location="pets"/><w:input><h:urlEncoded/></w:input><w:output><m:content type="application/xml"/><m:content type="text/xml"/></w:output></w:operation></w:binding>
      <w:service name="Pets"><w:port name="Main" binding="t:Binding"><h:address location="https://api.example.com/v1/"/></w:port></w:service>
    </w:definitions>"#;
    let report = convert(&request(source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("missing output")?;
    let operation = document
        .pointer("/paths/~1v1~1pets/get")
        .ok_or("missing operation")?;
    assert_eq!(operation["parameters"][0]["in"], "query");
    assert_eq!(operation["parameters"][1]["in"], "query");
    assert!(operation.get("requestBody").is_none());
    assert!(
        operation
            .pointer("/responses/2XX/content/text~1xml")
            .is_some()
    );
    let replacement = source
        .replace("location=\"pets\"", "location=\"pets/(id)\"")
        .replace("<h:urlEncoded/>", "<h:urlReplacement/>");
    let report = convert(&request(&replacement))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("missing output")?;
    let operation = document
        .pointer("/paths/~1v1~1pets~1{id}/get")
        .ok_or("missing replacement operation")?;
    assert_eq!(operation["parameters"][0]["in"], "path");
    assert_eq!(operation["parameters"].as_array().map(Vec::len), Some(1));
    assert!(operation.get("requestBody").is_none());
    Ok(())
}

#[test]
fn wsdl20_http_binding_preserves_safe_method_default_and_oneway_status()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"<description xmlns="http://www.w3.org/ns/wsdl" xmlns:t="urn:http" xmlns:x="http://www.w3.org/2001/XMLSchema" xmlns:h="http://www.w3.org/ns/wsdl/http" xmlns:ext="http://www.w3.org/ns/wsdl-extensions" targetNamespace="urn:http">
      <types><x:schema targetNamespace="urn:http"><x:element name="Empty"><x:complexType/></x:element></x:schema></types>
      <interface name="Service"><operation name="Ping" pattern="http://www.w3.org/ns/wsdl/in-only" ext:safe="true"><input element="t:Empty"/></operation></interface>
      <binding name="Binding" interface="t:Service" type="http://www.w3.org/ns/wsdl/http"><operation ref="t:Ping" h:location="ping"/></binding>
      <service name="Service" interface="t:Service"><endpoint name="Main" binding="t:Binding" address="https://api.example.com/v2/"/></service>
    </description>"#;
    let report = convert(&request(source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("missing output")?;
    assert_eq!(
        document.pointer("/paths/~1v2~1ping/get/responses/202/description"),
        Some(&json!("Declared HTTP success"))
    );
    Ok(())
}

#[test]
fn wsdl20_http_auth_headers_and_fault_codes_survive_native_conversion()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"<description xmlns="http://www.w3.org/ns/wsdl" xmlns:t="urn:http" xmlns:x="http://www.w3.org/2001/XMLSchema" xmlns:h="http://www.w3.org/ns/wsdl/http" targetNamespace="urn:http">
      <types><x:schema targetNamespace="urn:http"><x:element name="Text" type="x:string"/></x:schema></types>
      <interface name="Service"><fault name="Missing" element="t:Text"/><operation name="Ping" pattern="http://www.w3.org/ns/wsdl/in-out"><input element="t:Text"/><output element="t:Text"/><outfault ref="t:Missing"/></operation></interface>
      <binding name="Binding" interface="t:Service" type="http://www.w3.org/ns/wsdl/http"><fault ref="t:Missing" h:code="404"/><operation ref="t:Ping" h:location="ping"><input><h:header name="X-Request-ID" type="x:string" required="true"/></input><output><h:header name="X-Response-ID" type="x:string"/></output></operation></binding>
      <service name="Service" interface="t:Service"><endpoint name="Main" binding="t:Binding" address="https://api.example.com/v2/" h:authenticationScheme="basic" h:authenticationRealm="pets"/></service>
    </description>"#;
    let report = convert(&request(source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("missing output")?;
    let operation = document
        .pointer("/paths/~1v2~1ping/post")
        .ok_or("missing operation")?;
    assert_eq!(operation["parameters"][0]["name"], "X-Request-ID");
    assert_eq!(operation["parameters"][0]["required"], true);
    assert!(
        operation
            .pointer("/responses/404/content/application~1xml/schema/$ref")
            .is_some()
    );
    assert!(
        operation
            .pointer("/responses/2XX/headers/X-Response-ID/schema")
            .is_some()
    );
    assert_eq!(operation["x-http-authentication-realm"], "pets");
    let security = operation["security"][0]
        .as_object()
        .and_then(|map| map.keys().next())
        .ok_or("missing security")?;
    assert_eq!(
        document["components"]["securitySchemes"][security]["scheme"],
        "basic"
    );
    Ok(())
}

#[test]
fn explicit_remapping_can_replace_non_http_wsdl_transport() -> Result<(), Box<dyn std::error::Error>>
{
    let source=soap_service("http://schemas.xmlsoap.org/wsdl/soap/",r#"<w:operation name="Echo"><s:operation soapAction="urn:echo"/><w:input><s:body use="literal"/></w:input><w:output><s:body use="literal"/></w:output></w:operation>"#,r#"<w:operation name="Echo"><w:input message="t:Text"/><w:output message="t:Text"/></w:operation>"#).replace("https://api.example.com/soap","tcp://internal.example.com:8000/soap");
    let mut input = request(&source);
    let blocked = convert(&input)?;
    assert_eq!(blocked.status, Status::Blocked);
    input.config.http_mappings.insert("{urn:soap}Echo.Echo".into(),serde_json::from_value(json!({"method":"POST","path":"/echo","server":"https://api.example.com","responseStatus":"200","security":[]}))?);
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(
        report
            .document
            .as_ref()
            .and_then(|document| document.pointer("/paths/~1echo/post"))
            .is_some()
    );
    Ok(())
}
