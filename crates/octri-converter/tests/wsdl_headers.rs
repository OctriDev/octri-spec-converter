//! WSDL 2.0 SOAP headers reference global elements, unlike WSDL 1.1 parts.
use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::json;

const SOURCE: &str = r##"<description xmlns="http://www.w3.org/ns/wsdl" xmlns:t="urn:headers" xmlns:s="http://www.w3.org/ns/wsdl/soap" xmlns:h="http://www.w3.org/ns/wsdl/http" xmlns:x="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:headers">
  <types><x:schema targetNamespace="urn:headers"><x:element name="Auth" type="x:string"/><x:element name="Trace" type="x:string"/></x:schema></types>
  <interface name="Api"><operation name="Call" pattern="http://www.w3.org/ns/wsdl/in-out"><input element="#none"/><output element="#none"/></operation></interface>
  <binding name="Binding" interface="t:Api" type="http://www.w3.org/ns/wsdl/soap" s:protocol="http://www.w3.org/2003/05/soap/bindings/HTTP/"><operation ref="t:Call"><input><s:header element="t:Auth" required="true" mustUnderstand="1"/><s:header element="t:Trace"/></input><output><s:header element="t:Trace" required="1"/></output></operation></binding>
  <service name="Api" interface="t:Api"><endpoint name="Main" binding="t:Binding" address="https://api.example.test/soap" h:authenticationScheme="basic"/></service>
</description>"##;

fn request(content: &str) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://headers/service.wsdl".into(),
            content: Some(content.into()),
            generated: false,
        },
        files: Default::default(),
        config: Config {
            api_version: Some("1".into()),
            ..Default::default()
        },
    }
}

#[test]
fn soap20_element_headers_preserve_required_and_optional_blocks()
-> Result<(), Box<dyn std::error::Error>> {
    let report = convert(&request(SOURCE))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    let output = report.document.ok_or("output")?;
    let operation = &output["paths"]["/soap"]["post"];
    let input = &operation["requestBody"]["content"]["application/soap+xml"]["schema"];
    let headers = &input["properties"]["Header"];
    assert_eq!(headers["required"], json!(["{urn:headers}Auth"]));
    assert_eq!(
        headers["properties"]["{urn:headers}Auth"]["x-soap-must-understand"],
        true
    );
    assert!(headers["properties"]["{urn:headers}Trace"].is_object());
    assert!(
        input["required"]
            .as_array()
            .ok_or("required")?
            .contains(&json!("Header"))
    );
    let response = &operation["responses"]["2XX"]["content"]["application/soap+xml"]["schema"];
    assert_eq!(
        response["properties"]["Header"]["required"],
        json!(["{urn:headers}Trace"])
    );
    assert!(
        response["required"]
            .as_array()
            .ok_or("required")?
            .contains(&json!("Header"))
    );
    Ok(())
}

#[test]
fn optional_headers_do_not_require_the_header_container() -> Result<(), Box<dyn std::error::Error>>
{
    let report = convert(&request(
        &SOURCE
            .replace("required=\"true\"", "required=\"false\"")
            .replace("required=\"1\"", "required=\"0\""),
    ))?;
    let output = report.document.ok_or("output")?;
    let input = &output["paths"]["/soap"]["post"]["requestBody"]["content"]["application/soap+xml"]
        ["schema"];
    assert_eq!(input["required"], json!(["Body"]));
    assert!(input["properties"]["Header"].get("required").is_none());
    Ok(())
}

#[test]
fn soap_response_preserves_output_headers_and_rejects_outbound_soap_headers()
-> Result<(), Box<dyn std::error::Error>> {
    let content = SOURCE.replace(
        "s:protocol=",
        "s:mepDefault=\"http://www.w3.org/2003/05/soap/mep/soap-response/\" s:protocol=",
    );
    let error = convert(&request(&content))
        .err()
        .ok_or("outbound SOAP-Response header accepted")?;
    assert_eq!(error.code, "SOAP_HEADER");
    let content = content.replace(r#"<input><s:header element="t:Auth" required="true" mustUnderstand="1"/><s:header element="t:Trace"/></input>"#, "<input/>");
    let report = convert(&request(&content))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let output = report.document.ok_or("output")?;
    let operation = &output["paths"]["/soap"]["get"];
    assert!(operation.get("requestBody").is_none());
    assert_eq!(
        operation["responses"]["2XX"]["content"]["application/soap+xml"]["schema"]["properties"]["Header"]
            ["required"],
        json!(["{urn:headers}Trace"])
    );
    Ok(())
}

#[test]
fn fault_headers_stay_with_the_fault_that_declares_them() -> Result<(), Box<dyn std::error::Error>>
{
    let content = SOURCE.replace("<interface name=\"Api\">", r#"<interface name="Api"><fault name="Failed" element="t:Auth"/><fault name="Other" element="t:Trace"/>"#)
        .replace("<output element=\"#none\"/>", r##"<output element="#none"/><outfault ref="t:Failed"/><outfault ref="t:Other"/>"##)
        .replace("<operation ref=\"t:Call\">", r#"<fault ref="t:Failed"><s:header element="t:Auth" required="true"/></fault><operation ref="t:Call">"#);
    let report = convert(&request(&content))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let output = report.document.ok_or("output")?;
    let variants = output["paths"]["/soap"]["post"]["responses"]["500"]["content"]["application/soap+xml"]["schema"]["anyOf"].as_array().ok_or("fault alternatives")?;
    assert_eq!(
        variants.len(),
        3,
        "Two application faults plus protocol-fault fallback"
    );
    assert_eq!(
        variants[0]["properties"]["Header"]["required"],
        json!(["{urn:headers}Auth"])
    );
    assert!(variants[1]["properties"].get("Header").is_none());
    assert_eq!(variants[1]["required"], json!(["Body"]));
    assert!(variants[2]["properties"].get("Header").is_none());
    for replacement in [
        "element=\"t:Missing\" required=\"true\"",
        "element=\"t:Auth\" required=\"sometimes\"",
    ] {
        let invalid = content.replace(
            "<s:header element=\"t:Auth\" required=\"true\"/>",
            &format!("<s:header {replacement}/>"),
        );
        assert!(
            convert(&request(&invalid)).is_err(),
            "Invalid fault header must be checked"
        );
    }
    Ok(())
}

#[test]
fn protocol_faults_need_no_application_header_and_cannot_bypass_application_requirements()
-> Result<(), Box<dyn std::error::Error>> {
    let content = SOURCE.replace("<interface name=\"Api\">", r#"<interface name="Api"><fault name="Failed" element="t:Auth"/>"#)
        .replace("<output element=\"#none\"/>", r##"<output element="#none"/><outfault ref="t:Failed"/>"##)
        .replace("<operation ref=\"t:Call\">", r#"<fault ref="t:Failed"><s:header element="t:Auth" required="true"/></fault><operation ref="t:Call">"#);
    let report = convert(&request(&content))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let output = report.document.ok_or("output")?;
    let registry = jsonschema::Registry::new()
        .add(
            "memory://faults/openapi.json",
            jsonschema::Resource::from_contents(output),
        )?
        .prepare()?;
    let validator = jsonschema::options().with_registry(&registry).build(&json!({"$ref":"memory://faults/openapi.json#/paths/~1soap/post/responses/500/content/application~1soap+xml/schema"}))?;
    let mut fault = json!({"Body":{"payload":{"Code":{"Value":"env:VersionMismatch"},"Reason":{"Text":"Unsupported version"}}}});
    assert!(
        validator.is_valid(&fault),
        "Protocol fault requires no application header"
    );
    fault["Body"]["payload"]["Detail"] = json!("Application failure");
    assert!(
        !validator.is_valid(&fault),
        "Application detail cannot enter generic fallback without required header"
    );
    fault["Header"] = json!({"{urn:headers}Auth":"token"});
    assert!(
        validator.is_valid(&fault),
        "Required application header restores validity"
    );
    Ok(())
}

#[test]
fn invalid_header_declarations_cannot_silently_change_contracts()
-> Result<(), Box<dyn std::error::Error>> {
    for (content, code) in [
        (
            SOURCE.replace("element=\"t:Auth\"", "element=\"t:Missing\""),
            "WSDL_REFERENCE",
        ),
        (
            SOURCE.replace("required=\"true\"", "required=\"sometimes\""),
            "SOAP_HEADER",
        ),
        (
            SOURCE.replace("mustUnderstand=\"1\"", "mustUnderstand=\"perhaps\""),
            "SOAP_HEADER",
        ),
        (
            SOURCE.replace(
                "<s:header element=\"t:Trace\"/>",
                "<s:header element=\"t:Auth\"/>",
            ),
            "SOAP_HEADER",
        ),
    ] {
        let error = convert(&request(&content))
            .err()
            .ok_or("invalid header accepted")?;
        assert_eq!(error.code, code);
    }
    Ok(())
}
