use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::{Value, json};

const BASE: &str = include_str!("../../../fixtures/wsdl/inheritance-base.wsdl");
const DERIVED: &str = include_str!("../../../fixtures/wsdl/inheritance.wsdl");

fn request(source: &str) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://wsdl/service.wsdl".into(),
            content: Some(source.into()),
            generated: false,
        },
        files: std::collections::BTreeMap::from([("base.wsdl".into(), BASE.into())]),
        config: Config {
            api_version: Some("1".into()),
            ..Default::default()
        },
    }
}

fn expanded(document: &Value) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(octri_parser::execute(
        octri_parser::Request {
            operation: octri_parser::Operation::Dereference,
            input: Input {
                uri: "memory://relocated/openapi.json".into(),
                content: Some(document.to_string()),
                generated: true,
            },
            options: Default::default(),
        },
        &mut octri_parser::NoIo,
    )?
    .graph
    .ok_or("missing graph")?
    .to_value(&Default::default())?)
}

#[test]
fn inherited_http_operations_and_faults_keep_namespace_and_diamond_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let report = convert(&request(DERIVED))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let output = expanded(report.document.as_ref().ok_or("missing output")?)?;
    assert_eq!(output["paths"].as_object().ok_or("paths")?.len(), 2);
    for path in ["find", "count"] {
        assert_eq!(
            output.pointer(&format!(
                "/paths/~1v2~1{path}/get/responses/404/content/application~1xml/schema/type"
            )),
            Some(&json!("string")),
            "Inherited fault QName must select base string, not derived integer"
        );
    }
    assert!(
        report
            .decisions
            .iter()
            .any(|item| item.code == "WSDL_INTERFACE_INHERITANCE"
                && item.source.address.uri == "memory://wsdl/service.wsdl")
    );
    Ok(())
}

#[test]
fn inherited_soap_faults_resolve_against_original_interface()
-> Result<(), Box<dyn std::error::Error>> {
    let source = DERIVED
        .replace("xmlns:h=", "xmlns:s=\"http://www.w3.org/ns/wsdl/soap\" xmlns:h=")
        .replace("type=\"http://www.w3.org/ns/wsdl/http\"", "type=\"http://www.w3.org/ns/wsdl/soap\" s:protocol=\"http://www.w3.org/2003/05/soap/bindings/HTTP/\"")
        .replace("<operation ref=\"b:Find\" h:location=\"find\"/>", "<operation ref=\"b:Find\" s:action=\"urn:find\"/>")
        .replace("<operation ref=\"t:Count\" h:location=\"count\"/>", "");
    let report = convert(&request(&source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(
        !report
            .decisions
            .iter()
            .any(|item| item.code == "WSDL_OVERLOADED_BINDING")
    );
    let output = expanded(report.document.as_ref().ok_or("missing output")?)?;
    let alternatives = output
        .pointer("/paths/~1v2~1/post/responses/500/content/application~1soap+xml/schema/anyOf")
        .and_then(Value::as_array)
        .ok_or("missing merged SOAP faults")?;
    // Each operation now keeps application and protocol faults separate.
    assert_eq!(
        alternatives.len(),
        2,
        "Explicit and default-bound operations share endpoint"
    );
    for operation in alternatives {
        let variants = operation["anyOf"].as_array().ok_or("fault variants")?;
        assert_eq!(variants.len(), 2);
        let schema = &variants[0];
        let detail = schema
            .pointer("/properties/Body/properties/payload/properties/Detail/oneOf/0/type")
            .ok_or("missing SOAP fault detail")?;
        assert_eq!(detail, &json!("string"));
    }
    Ok(())
}

#[test]
fn invalid_interface_graphs_cannot_be_acknowledged_into_output()
-> Result<(), Box<dyn std::error::Error>> {
    for (interfaces, code) in [
        (
            "<interface name=\"A\" extends=\"t:A\"/>",
            "WSDL_INTERFACE_CYCLE",
        ),
        (
            "<interface name=\"A\" extends=\"t:B\"/><interface name=\"B\" extends=\"t:A\"/>",
            "WSDL_INTERFACE_CYCLE",
        ),
        (
            "<interface name=\"A\" extends=\"t:Missing\"/>",
            "WSDL_REFERENCE",
        ),
        (
            "<interface name=\"A\" extends=\"t:B t:B\"/><interface name=\"B\"/>",
            "WSDL_INTERFACE_DUPLICATE_BASE",
        ),
    ] {
        let source = format!(
            "<description xmlns=\"http://www.w3.org/ns/wsdl\" xmlns:t=\"urn:test\" targetNamespace=\"urn:test\">{interfaces}</description>"
        );
        assert_eq!(
            convert(&request(&source))
                .err()
                .ok_or("accepted invalid inheritance")?
                .code,
            code
        );
    }
    Ok(())
}

#[test]
fn conflicting_inherited_qnames_require_explicit_resolution()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r##"<description xmlns="http://www.w3.org/ns/wsdl" xmlns:t="urn:test" targetNamespace="urn:test">
      <interface name="A"><operation name="Ping" pattern="http://www.w3.org/ns/wsdl/in-only"><input element="#none"/></operation></interface>
      <interface name="B"><operation name="Ping" pattern="http://www.w3.org/ns/wsdl/in-out"><input element="#none"/><output element="#none"/></operation></interface>
      <interface name="Service" extends="t:A t:B"/>
    </description>"##;
    assert_eq!(
        convert(&request(source))
            .err()
            .ok_or("accepted conflicting operations")?
            .code,
        "WSDL_INTERFACE_CONFLICT"
    );
    Ok(())
}

#[test]
fn identical_members_with_distinct_declaring_parents_are_not_equivalent()
-> Result<(), Box<dyn std::error::Error>> {
    // WSDL 2.0 component equivalence includes the required {parent} property.
    // Equal payload XML cannot make different declaring interfaces equivalent.
    for member in [
        r##"<operation name="Ping" pattern="http://www.w3.org/ns/wsdl/in-only"><input element="#none"/></operation>"##,
        r##"<fault name="Failure" element="#none"/>"##,
    ] {
        let source = format!(
            r#"<description xmlns="http://www.w3.org/ns/wsdl" xmlns:t="urn:test" targetNamespace="urn:test">
          <interface name="A">{member}</interface>
          <interface name="B">{member}</interface>
          <interface name="Service" extends="t:A t:B"/>
        </description>"#
        );
        let error = convert(&request(&source))
            .err()
            .ok_or("invalid parent equivalence accepted")?;
        assert_eq!(error.code, "WSDL_INTERFACE_CONFLICT");
    }
    Ok(())
}

#[test]
fn interface_expansion_respects_depth_and_total_work_limits()
-> Result<(), Box<dyn std::error::Error>> {
    let mut interfaces = String::new();
    for index in 0..16 {
        interfaces.push_str(&format!(
            "<interface name=\"I{index:02}\"{} />",
            if index == 15 {
                String::new()
            } else {
                format!(" extends=\"t:I{:02}\"", index + 1)
            }
        ));
    }
    let source = format!(
        "<description xmlns=\"http://www.w3.org/ns/wsdl\" xmlns:t=\"urn:test\" targetNamespace=\"urn:test\">{interfaces}</description>"
    );
    for depth_limit in [true, false] {
        let mut input = request(&source);
        if depth_limit {
            input.config.limits.max_depth = 8;
        } else {
            input.config.limits.max_nodes = 100;
        }
        assert_eq!(
            convert(&input)
                .err()
                .ok_or("accepted excessive expansion")?
                .code,
            "WSDL_INTERFACE_LIMIT"
        );
    }
    // Reverse the dependency direction so each base has already been cached.
    let mut reversed = source;
    for index in 0..16 {
        reversed = reversed.replace(&format!("I{index:02}"), &format!("R{:02}", 15 - index));
    }
    let mut input = request(&reversed);
    input.config.limits.max_depth = 8;
    assert_eq!(
        convert(&input)
            .err()
            .ok_or("cached ancestry bypassed depth")?
            .code,
        "WSDL_INTERFACE_LIMIT"
    );
    Ok(())
}
