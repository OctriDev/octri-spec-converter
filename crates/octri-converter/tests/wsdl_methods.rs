//! WSDL HTTP verbs are case-sensitive source contracts, not display labels.
use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;

fn service(version: &str, method: &str) -> String {
    if version == "1.1" {
        format!(
            r#"<definitions xmlns="http://schemas.xmlsoap.org/wsdl/" xmlns:t="urn:methods" xmlns:h="http://schemas.xmlsoap.org/wsdl/http/" targetNamespace="urn:methods">
          <portType name="Api"><operation name="Call"/></portType>
          <binding name="Http" type="t:Api"><h:binding verb="{method}"/><operation name="Call"><h:operation location="call"/></operation></binding>
          <service name="Api"><port name="Http" binding="t:Http"><h:address location="https://api.example.test/v1/"/></port></service>
        </definitions>"#
        )
    } else {
        format!(
            r##"<description xmlns="http://www.w3.org/ns/wsdl" xmlns:t="urn:methods" xmlns:h="http://www.w3.org/ns/wsdl/http" targetNamespace="urn:methods">
          <interface name="Api"><operation name="Call" pattern="http://www.w3.org/ns/wsdl/in-only"><input element="#none"/></operation></interface>
          <binding name="Http" interface="t:Api" type="http://www.w3.org/ns/wsdl/http" h:methodDefault="{method}"><operation ref="t:Call" h:location="call"/></binding>
          <service name="Api" interface="t:Api"><endpoint name="Http" binding="t:Http" address="https://api.example.test/v1/"/></service>
        </description>"##
        )
    }
}

fn request(content: String, target: &str) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://methods/service.wsdl".into(),
            content: Some(content),
            generated: false,
        },
        files: Default::default(),
        config: Config {
            target_version: Some(target.into()),
            api_version: Some("1".into()),
            ..Default::default()
        },
    }
}

#[test]
fn native_wsdl_bindings_preserve_exact_http_method_and_target_limits()
-> Result<(), Box<dyn std::error::Error>> {
    for version in ["1.1", "2.0"] {
        for method in ["GET", "get", "QUERY", "COPY", "copy", "~COPY"] {
            let report = convert(&request(service(version, method), "3.2.1"))?;
            assert!(report.valid, "{version} {method}: {:?}", report.diagnostics);
            assert_ne!(
                report.status,
                Status::Blocked,
                "{version} {method}: {:?}",
                report.decisions
            );
            let document = report.document.ok_or("missing output")?;
            let item = &document["paths"]["/v1/call"];
            let operation = match method {
                "GET" => &item["get"],
                "QUERY" => &item["query"],
                _ => &item["additionalOperations"][method],
            };
            assert!(operation.is_object(), "{version} {method}: {item}");
            if method == "get" {
                assert!(
                    item.get("get").is_none(),
                    "Lowercase get must not become GET"
                );
            }
            if method != "GET" {
                let legacy = convert(&request(service(version, method), "3.1.2"))?;
                assert_eq!(legacy.status, Status::Blocked, "{version} {method}");
                assert!(!legacy.handoff_allowed);
            }
        }
    }
    Ok(())
}

#[test]
fn wsdl2_operation_method_overrides_binding_default_without_normalization()
-> Result<(), Box<dyn std::error::Error>> {
    let source = service("2.0", "POST").replace(
        "h:location=\"call\"",
        "h:location=\"call\" h:method=\"get\"",
    );
    let report = convert(&request(source, "3.2.1"))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("missing output")?;
    assert!(document["paths"]["/v1/call"]["additionalOperations"]["get"].is_object());
    Ok(())
}

#[test]
fn case_different_wsdl_endpoints_at_same_path_stay_separate()
-> Result<(), Box<dyn std::error::Error>> {
    let source = service("2.0", "GET").replace("</description>", r#"
        <binding name="Lower" interface="t:Api" type="http://www.w3.org/ns/wsdl/http" h:methodDefault="get"><operation ref="t:Call" h:location="call"/></binding>
        <service name="Lower" interface="t:Api"><endpoint name="Http" binding="t:Lower" address="https://api.example.test/v1/"/></service>
        </description>"#);
    let report = convert(&request(source, "3.2.1"))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("missing output")?;
    let item = &document["paths"]["/v1/call"];
    assert!(item["get"].is_object());
    assert!(item["additionalOperations"]["get"].is_object());
    assert_ne!(
        item["get"]["operationId"],
        item["additionalOperations"]["get"]["operationId"]
    );
    assert!(item["get"].get("x-wsdl-variants").is_none());
    Ok(())
}

#[test]
fn invalid_wsdl_method_tokens_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    for version in ["1.1", "2.0"] {
        for method in ["", "BAD METHOD", "GET/OTHER"] {
            let error = convert(&request(service(version, method), "3.2.1"))
                .err()
                .ok_or("invalid token accepted")?;
            assert_eq!(error.code, "WSDL_HTTP_METHOD", "{version} {method}");
        }
    }
    Ok(())
}

#[test]
fn custom_lowercase_get_does_not_inherit_get_serialization_defaults()
-> Result<(), Box<dyn std::error::Error>> {
    for method in ["GET", "get"] {
        let source = service("2.0", method)
            .replace("xmlns:t=", "xmlns:x=\"http://www.w3.org/2001/XMLSchema\" xmlns:m=\"urn:models\" xmlns:t=")
            .replace("<interface name=", r#"<types><x:schema targetNamespace="urn:models"><x:element name="Request"><x:complexType><x:sequence><x:element name="value" type="x:string"/></x:sequence></x:complexType></x:element></x:schema></types><interface name="#)
            .replace("element=\"#none\"", "element=\"m:Request\"");
        let report = convert(&request(source, "3.2.1"))?;
        assert!(report.valid, "{:?}", report.diagnostics);
        let document = report.document.ok_or("missing output")?;
        if method == "GET" {
            let operation = &document["paths"]["/v1/call"]["get"];
            assert_eq!(operation["parameters"][0]["in"], "query");
            assert_eq!(operation["parameters"][0]["name"], "value");
            assert!(operation.get("requestBody").is_none());
        } else {
            let operation = &document["paths"]["/v1/call"]["additionalOperations"]["get"];
            assert!(operation["requestBody"]["content"]["application/xml"].is_object());
            assert!(operation.get("parameters").is_none());
        }
    }
    Ok(())
}
