//! WSDL 2.0 defaults apply without manufacturing operation declarations or URLs.
use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::{Value, json};

fn source(binding: &str, operations: &str) -> String {
    format!(
        r##"<description xmlns="http://www.w3.org/ns/wsdl" xmlns:t="urn:defaults" xmlns:x="http://www.w3.org/2001/XMLSchema" xmlns:h="http://www.w3.org/ns/wsdl/http" xmlns:s="http://www.w3.org/ns/wsdl/soap" xmlns:wx="http://www.w3.org/ns/wsdl-extensions" targetNamespace="urn:defaults">
          <types><x:schema targetNamespace="urn:defaults"><x:element name="Request"><x:complexType><x:sequence><x:element name="term" type="x:string"/></x:sequence></x:complexType></x:element><x:element name="Reply" type="x:string"/></x:schema></types>
          <interface name="Api">{operations}</interface>
          {binding}
          <service name="Api" interface="t:Api"><endpoint name="Main" binding="t:Binding" address="https://api.example.test/v1/" h:authenticationScheme="basic"/></service>
        </description>"##
    )
}

fn operation(name: &str, safe: bool) -> String {
    format!(
        r#"<operation name="{name}" pattern="http://www.w3.org/ns/wsdl/in-out" wx:safe="{safe}"><input element="t:Request"/><output element="t:Reply"/></operation>"#
    )
}

fn request(content: String) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://defaults/service.wsdl".into(),
            content: Some(content),
            generated: false,
        },
        files: Default::default(),
        config: Config {
            target_version: Some("3.2.1".into()),
            api_version: Some("1".into()),
            ..Default::default()
        },
    }
}

fn document(content: String) -> Result<Value, Box<dyn std::error::Error>> {
    let report = convert(&request(content))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    assert!(
        !report
            .decisions
            .iter()
            .any(|item| item.code == "HTTP_MAPPING_REQUIRED"),
        "{:?}",
        report.decisions
    );
    Ok(report.document.ok_or("missing output")?)
}

#[test]
fn http_defaults_choose_safe_get_and_unsafe_post_at_exact_endpoint()
-> Result<(), Box<dyn std::error::Error>> {
    let output = document(source(
        r#"<binding name="Binding" interface="t:Api" type="http://www.w3.org/ns/wsdl/http"/>"#,
        &(operation("Read", true) + &operation("Write", false)),
    ))?;
    let paths = output["paths"].as_object().ok_or("paths")?;
    assert_eq!(paths.len(), 1, "Never invent an operation-name URL");
    let item = &paths["/v1/"];
    assert_eq!(item["get"]["parameters"][0]["name"], "term");
    assert_eq!(item["get"]["parameters"][0]["in"], "query");
    assert!(item["get"].get("requestBody").is_none());
    assert!(item["post"]["requestBody"]["content"]["application/xml"].is_object());
    assert!(item["get"]["security"][0].is_object());
    Ok(())
}

#[test]
fn reusable_binding_applies_defaults_to_each_service_interface()
-> Result<(), Box<dyn std::error::Error>> {
    let content = source(
        r#"<binding name="Binding" type="http://www.w3.org/ns/wsdl/http" h:methodDefault="QUERY"/>"#,
        &operation("Read", true),
    ).replace("</description>", &format!(
        r#"<interface name="Other">{}</interface><service name="Other" interface="t:Other"><endpoint name="Main" binding="t:Binding" address="https://api.example.test/other/" h:authenticationScheme="basic"/></service></description>"#,
        operation("OtherRead", false)
    ));
    let output = document(content)?;
    assert!(output["paths"]["/v1/"]["query"].is_object());
    assert!(output["paths"]["/other/"]["query"].is_object());
    Ok(())
}

#[test]
fn explicit_override_does_not_remove_defaults_for_inherited_members()
-> Result<(), Box<dyn std::error::Error>> {
    let content = source(
        r#"<binding name="Binding" interface="t:Api" type="http://www.w3.org/ns/wsdl/http" h:methodDefault="GET"><operation ref="t:Read" h:method="DELETE" h:location="read"/></binding>"#,
        &operation("Read", true),
    ).replace("<interface name=\"Api\">", &format!(
        r#"<interface name="Base">{}</interface><interface name="Api" extends="t:Base">"#,
        operation("Inherited", false)
    ));
    let output = document(content)?;
    assert!(output["paths"]["/v1/read"]["delete"].is_object());
    assert!(output["paths"]["/v1/"]["get"].is_object());
    assert!(output["paths"]["/v1/"]["delete"].is_null());
    Ok(())
}

#[test]
fn duplicate_operation_bindings_and_incompatible_service_interfaces_are_invalid()
-> Result<(), Box<dyn std::error::Error>> {
    for (binding, code) in [
        (
            r#"<binding name="Binding" interface="t:Api" type="http://www.w3.org/ns/wsdl/http"><operation ref="t:Read"/><operation ref="t:Read"/></binding>"#,
            "WSDL_DUPLICATE_BINDING_OPERATION",
        ),
        (
            r#"<interface name="Other"/><binding name="Binding" interface="t:Other" type="http://www.w3.org/ns/wsdl/http"/>"#,
            "WSDL_BINDING_INTERFACE",
        ),
        (
            r#"<binding name="Binding" type="http://www.w3.org/ns/wsdl/http"><operation ref="t:Read"/></binding>"#,
            "WSDL_REUSABLE_BINDING",
        ),
    ] {
        let error = convert(&request(source(binding, &operation("Read", true))))
            .err()
            .ok_or("invalid binding accepted")?;
        assert_eq!(error.code, code);
    }
    Ok(())
}

#[test]
fn soap_request_response_defaults_keep_envelopes_without_inventing_action()
-> Result<(), Box<dyn std::error::Error>> {
    let output = document(source(
        r#"<binding name="Binding" interface="t:Api" type="http://www.w3.org/ns/wsdl/soap" s:protocol="http://www.w3.org/2003/05/soap/bindings/HTTP/"/>"#,
        &operation("Read", true),
    ))?;
    let call = &output["paths"]["/v1/"]["post"];
    assert_eq!(call["x-wsdl-soap"]["version"], "1.2");
    assert_eq!(call["x-wsdl-soap"]["action"], Value::Null);
    assert!(
        call["requestBody"]["content"]["application/soap+xml"]["schema"]["x-soap-envelope"]
            == json!(true)
    );
    assert!(call.get("parameters").is_none());
    Ok(())
}

#[test]
fn soap_response_default_uses_get_query_input_and_soap_output()
-> Result<(), Box<dyn std::error::Error>> {
    let binding = r#"<binding name="Binding" interface="t:Api" type="http://www.w3.org/ns/wsdl/soap" s:protocol="http://www.w3.org/2003/05/soap/bindings/HTTP/" s:mepDefault="http://www.w3.org/2003/05/soap/mep/soap-response/" h:methodDefault="DELETE"><operation ref="t:Read" s:action="urn:ignored" h:location="find" h:inputSerialization="application/xml"/></binding>"#;
    let output = document(source(binding, &operation("Read", false)))?;
    let call = &output["paths"]["/v1/find"]["get"];
    assert_eq!(call["parameters"][0]["name"], "term");
    assert_eq!(call["parameters"][0]["in"], "query");
    assert!(call.get("requestBody").is_none());
    assert!(call["x-wsdl-soap"]["action"].is_null());
    assert_eq!(
        call["responses"]["2XX"]["content"]["application/soap+xml"]["schema"]["x-soap-envelope"],
        true
    );
    assert!(
        call["responses"]["2XX"]["content"]
            .get("application/xml")
            .is_none()
    );
    let implicit = binding.replace(r#"<operation ref="t:Read" s:action="urn:ignored" h:location="find" h:inputSerialization="application/xml"/>"#, "");
    let output = document(source(&implicit, &operation("Read", false)))?;
    assert!(output["paths"]["/v1/"]["get"].is_object());
    Ok(())
}

#[test]
fn operation_mep_overrides_default_and_request_location_is_preserved()
-> Result<(), Box<dyn std::error::Error>> {
    let output = document(source(
        r#"<binding name="Binding" interface="t:Api" type="http://www.w3.org/ns/wsdl/soap" s:protocol="http://www.w3.org/2003/05/soap/bindings/HTTP/" s:mepDefault="http://www.w3.org/2003/05/soap/mep/soap-response/"><operation ref="t:Read" s:mep="http://www.w3.org/2003/05/soap/mep/request-response/" h:location="read"/></binding>"#,
        &operation("Read", true),
    ))?;
    assert!(output["paths"]["/v1/read"]["post"]["requestBody"].is_object());
    assert!(output["paths"]["/v1/read"]["get"].is_null());
    Ok(())
}

#[test]
fn invalid_or_unsupported_message_patterns_never_silently_become_post()
-> Result<(), Box<dyn std::error::Error>> {
    let soap = r#"<binding name="Binding" interface="t:Api" type="http://www.w3.org/ns/wsdl/soap" s:protocol="http://www.w3.org/2003/05/soap/bindings/HTTP/"/>"#;
    for (binding, operations, code) in [
        (
            soap.to_owned(),
            operation("Read", false).replace("/in-out", "/in-only"),
            "WSDL_SOAP_MEP",
        ),
        (
            soap.replace("s:protocol=", "s:mepDefault=\"urn:custom\" s:protocol="),
            operation("Read", false),
            "WSDL_SOAP_MEP",
        ),
        (
            soap.replace("s:protocol=", "s:version=\"1.1\" s:protocol="),
            operation("Read", false),
            "WSDL_SOAP_VERSION",
        ),
        (
            r#"<binding name="Binding" interface="t:Api" type="http://www.w3.org/ns/wsdl/http"/>"#
                .into(),
            operation("Read", false).replace("/in-out", "/out-in"),
            "WSDL_HTTP_MEP",
        ),
    ] {
        let error = convert(&request(source(&binding, &operations)))
            .err()
            .ok_or("invalid binding accepted")?;
        assert_eq!(error.code, code);
    }
    Ok(())
}

#[test]
fn explicit_mapping_replaces_implicit_transport_with_physical_binding_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let content = source(
        r#"<binding name="Binding" interface="t:Api" type="http://www.w3.org/ns/wsdl/http"/>"#,
        &operation("Read", true),
    );
    let mut input = request(content.clone());
    input.config.http_mappings.insert("{urn:defaults}Api.Read".into(), serde_json::from_value(json!({
        "method":"POST","path":"/replacement","server":"https://replacement.example.test","responseStatus":"200","security":[]
    }))?);
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let evidence = report
        .decisions
        .iter()
        .find(|item| item.code == "WSDL_TRANSPORT_REMAPPED")
        .ok_or("remap evidence")?;
    assert_eq!(evidence.source.address.uri, input.input.uri);
    assert_eq!(
        evidence.source.offset,
        Some(content.find("<binding").ok_or("binding")?)
    );
    let output = report.document.ok_or("output")?;
    assert!(output["paths"]["/replacement"]["post"].is_object());
    assert!(output["paths"].get("/v1/").is_none());
    Ok(())
}

#[test]
fn repeated_endpoint_expansion_respects_total_node_work_limit()
-> Result<(), Box<dyn std::error::Error>> {
    let endpoints = (0..30).map(|index| format!(r#"<endpoint name="E{index}" binding="t:Binding" address="https://api.example.test/{index}/"/>"#)).collect::<String>();
    let operations = (0..30)
        .map(|index| operation(&format!("Op{index}"), true))
        .collect::<String>();
    let content = source(r#"<binding name="Binding" interface="t:Api" type="http://www.w3.org/ns/wsdl/http"/>"#, &operations)
        .replace(r#"<endpoint name="Main" binding="t:Binding" address="https://api.example.test/v1/" h:authenticationScheme="basic"/>"#, &endpoints);
    let mut input = request(content);
    input.config.limits.max_nodes = 500;
    let error = convert(&input).err().ok_or("unbounded binding expansion")?;
    assert_eq!(error.code, "WSDL_BINDING_LIMIT");
    Ok(())
}

#[test]
fn soap_response_rejects_any_content_model_in_either_direction()
-> Result<(), Box<dyn std::error::Error>> {
    let binding = r#"<binding name="Binding" interface="t:Api" type="http://www.w3.org/ns/wsdl/soap" s:protocol="http://www.w3.org/2003/05/soap/bindings/HTTP/" s:mepDefault="http://www.w3.org/2003/05/soap/mep/soap-response/"/>"#;
    for element in ["t:Request", "t:Reply"] {
        let invalid =
            operation("Read", false).replace(&format!("element=\"{element}\""), "element=\"#any\"");
        let error = convert(&request(source(binding, &invalid)))
            .err()
            .ok_or("SOAP-Response #any accepted")?;
        assert_eq!(error.code, "WSDL_SOAP_MEP");
    }
    Ok(())
}

#[test]
fn wsdl11_never_gets_wsdl2_implicit_operation_bindings() -> Result<(), Box<dyn std::error::Error>> {
    let report = convert(&request(r#"<definitions xmlns="http://schemas.xmlsoap.org/wsdl/" xmlns:t="urn:legacy" xmlns:h="http://schemas.xmlsoap.org/wsdl/http/" targetNamespace="urn:legacy"><portType name="Api"><operation name="Read"/></portType><binding name="Binding" type="t:Api"><h:binding verb="GET"/></binding><service name="Api"><port name="Main" binding="t:Binding"><h:address location="https://api.example.test/v1/"/></port></service></definitions>"#.into()))?;
    assert_eq!(report.status, Status::Blocked);
    assert!(
        report.document.ok_or("output")?["paths"]
            .as_object()
            .ok_or("paths")?
            .is_empty()
    );
    Ok(())
}
