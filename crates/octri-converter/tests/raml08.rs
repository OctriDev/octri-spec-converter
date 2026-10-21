//! Version-specific RAML 0.8 transport semantics must not inherit RAML 1.0 defaults.
use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::json;

fn request(source: &str) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://raml08/api.raml".into(),
            content: Some(source.into()),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    }
}

const SOURCE: &str = "#%RAML 0.8\ntitle: Legacy search\nversion: v1\nbaseUri: https://api.example.test/{version}\nsecuritySchemes:\n  - basic:\n      type: Basic Authentication\nsecuredBy: [basic]\ntraits:\n  - paged:\n      queryParameters:\n        limit:\n          type: integer\n          maximum: 100\nresourceTypes:\n  - collection:\n      get?:\n        is: [paged]\n        responses:\n          204:\n            description: Empty\n/search:\n  type: collection\n  get:\n    queryParameters:\n      tag:\n        repeat: true\n      exact?:\n        type: string\n      required:\n        required: true\n    headers:\n      X-Trace:\n        type: string\n";

#[test]
fn declaration_arrays_templates_and_optional_defaults_preserve_legacy_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let report = convert(&request(SOURCE))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let output = report.document.ok_or("output")?;
    let operation = &output["paths"]["/search"]["get"];
    assert!(operation["responses"]["204"].is_object());
    assert_eq!(output["servers"][0]["url"], "https://api.example.test/v1");
    assert_eq!(
        output["components"]["securitySchemes"]["basic"]["scheme"],
        "basic"
    );
    for parameter in operation["parameters"].as_array().ok_or("parameters")? {
        let name = parameter["name"].as_str().ok_or("name")?;
        assert_eq!(parameter["required"], name == "required", "{name}");
        if name == "tag" {
            assert_eq!(parameter["schema"]["type"], "array");
            assert_eq!(parameter["schema"]["items"]["type"], "string");
            assert_eq!(parameter["style"], "form");
            assert_eq!(parameter["explode"], true);
        }
    }
    assert!(
        operation["parameters"]
            .as_array()
            .ok_or("parameters")?
            .iter()
            .any(|p| p["name"] == "exact?")
    );
    Ok(())
}

#[test]
fn included_declaration_arrays_keep_physical_locations() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(&SOURCE.replace("  - paged:\n      queryParameters:\n        limit:\n          type: integer\n          maximum: 100", "  - !include traits/paged.raml"));
    input.files.insert(
        "traits/paged.raml".into(),
        "paged:\n  queryParameters:\n    limit:\n      type: integer\n      maximum: 100\n".into(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    assert!(report.decisions.iter().any(|d|d.source.address.uri.ends_with("traits/paged.raml") && d.source.line.is_some()));
    Ok(())
}

#[test]
fn logical_declaration_names_cannot_overwrite_unread_physical_origins()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "#%RAML 0.8\ntitle: Origin collision\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n  - \"0\":\n      queryParameters:\n        unused: {type: string}\n    queryParameters:\n      queryParameters:\n        used: {type: integer}\n/items:\n  get:\n    is: [queryParameters]\n    responses:\n      204: {description: Empty}\n";
    let report = convert(&request(source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let declarations = report
        .decisions
        .iter()
        .filter(|decision| decision.code == "RAML_LEGACY_DECLARATION")
        .map(|decision| decision.source.address.pointer.as_str())
        .collect::<Vec<_>>();
    assert_eq!(declarations, ["/traits/0/0", "/traits/0/queryParameters"]);
    let output = report.document.ok_or("output")?;
    assert_eq!(
        output["paths"]["/items"]["get"]["parameters"][0]["name"],
        "used"
    );
    Ok(())
}

#[test]
fn numeric_api_versions_keep_declared_value_in_metadata_and_server()
-> Result<(), Box<dyn std::error::Error>> {
    let report = convert(&request(&SOURCE.replace("version: v1", "version: 0.8")))?;
    let output = report.document.ok_or("output")?;
    assert_eq!(output["info"]["version"], "0.8");
    assert_eq!(output["servers"][0]["url"], "https://api.example.test/0.8");
    Ok(())
}

#[test]
fn legacy_repeated_and_alternative_form_fields_preserve_upload_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let input = "#%RAML 0.8\ntitle: Upload\nversion: v1\nbaseUri: https://api.example.test\n/files:\n  post:\n    body:\n      multipart/form-data:\n        formParameters:\n          file:\n            - type: string\n              required: true\n            - type: file\n              required: true\n          labels:\n            repeat: true\n    responses:\n      201:\n        description: Created\n";
    let report = convert(&request(input))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let output = report.document.ok_or("output")?;
    let schema = &output["paths"]["/files"]["post"]["requestBody"]["content"]["multipart/form-data"]
        ["schema"];
    assert_eq!(schema["required"], json!(["file"]));
    assert_eq!(schema["properties"]["file"]["anyOf"][1]["format"], "binary");
    assert_eq!(schema["properties"]["labels"]["type"], "array");
    Ok(())
}

#[test]
fn draft4_included_and_serialized_schemas_keep_validation_and_original_sources()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "#%RAML 0.8\ntitle: Shapes\nversion: v1\nbaseUri: https://api.example.test\nschemas:\n  - Payload: !include schemas/payload.json\n/items:\n  post:\n    body:\n      application/json:\n        schema: Payload\n    responses:\n      200:\n        body:\n          application/json:\n            schema: |\n              {\"$schema\":\"http://json-schema.org/draft-04/schema#\",\"definitions\":{\"Text\":{\"type\":\"string\"}},\"$ref\":\"#/definitions/Text\"}\n";
    let payload = json!({"$schema":"http://json-schema.org/draft-04/schema#","id":"https://schemas.example.test/payload.json","type":"object","definitions":{"Value":{"type":"number","minimum":0,"exclusiveMinimum":true}},"properties":{"value":{"$ref":"#/definitions/Value"},"other":{"$ref":"value.json"}},"required":["value"]});
    let mut input = request(source);
    input
        .files
        .insert("schemas/payload.json".into(), payload.to_string());
    input.files.insert(
        "https://schemas.example.test/value.json".into(),
        json!({"$schema":"http://json-schema.org/draft-04/schema#","type":"integer"}).to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let output = report.document.ok_or("output")?;
    let mut registry = jsonschema::Registry::new();
    for schema in output["components"]["schemas"]
        .as_object()
        .ok_or("schemas")?
        .values()
    {
        if let Some(id) = schema
            .get("id")
            .or_else(|| schema.get("$id"))
            .and_then(serde_json::Value::as_str)
        {
            registry = registry.add(id, jsonschema::Resource::from_contents(schema.clone()))?;
        }
    }
    // API containers are not JSON Schema resources. Register component resources
    // explicitly and resolve local component references through one API identity.
    let registry = registry
        .add(
            "memory://relocated/api.json",
            jsonschema::Resource::from_contents(output.clone()),
        )?
        .prepare()?;
    for (pointer, valid, invalid) in [
        (
            "/paths/~1items/post/requestBody/content/application~1json/schema",
            json!({"value":1,"other":2}),
            json!({"value":0}),
        ),
        (
            "/paths/~1items/post/responses/200/content/application~1json/schema",
            json!("ok"),
            json!(2),
        ),
    ] {
        let validator = jsonschema::options()
            .with_registry(&registry)
            .build(&json!({"$ref":format!("memory://relocated/api.json#{pointer}")}))?;
        assert!(validator.is_valid(&valid), "{pointer}");
        assert!(!validator.is_valid(&invalid), "{pointer}");
    }
    let serialized = serde_json::to_value(&report.manifest.files)?;
    assert!(
        !serialized.to_string().contains(".octri-schema-"),
        "Derived files must not masquerade as original sources"
    );
    assert!(
        report
            .decisions
            .iter()
            .any(|d| d.code == "RAML_BODY_SCHEMA" && d.source.line.is_some())
    );
    Ok(())
}

#[test]
fn unsupported_schema_dialects_are_blocked_without_activating_constraints()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "#%RAML 0.8\ntitle: Legacy\nversion: v1\nbaseUri: https://api.example.test\n/items:\n  post:\n    body:\n      application/json:\n        schema: '{\"$schema\":\"urn:custom-schema\",\"type\":\"object\"}'\n    responses:\n      204:\n        description: Empty\n";
    let report = convert(&request(source))?;
    assert_eq!(report.status, Status::Blocked);
    assert!(
        report
            .decisions
            .iter()
            .any(|d| d.code == "RAML_SCHEMA_DIALECT")
    );
    Ok(())
}

#[test]
fn embedded_schema_errors_locate_original_scalar_without_derived_addresses()
-> Result<(), Box<dyn std::error::Error>> {
    for schema in ["{", r##"{"$ref":"#/missing"}"##] {
        let quoted = serde_json::to_string(schema)?;
        let source = format!(
            "#%RAML 0.8\ntitle: Errors\nversion: v1\nbaseUri: https://api.example.test\n/items:\n  post:\n    body:\n      application/json:\n        schema: {quoted}\n    responses:\n      204: {{description: Empty}}\n"
        );
        let error = convert(&request(&source))
            .err()
            .ok_or("invalid schema accepted")?;
        assert_eq!(error.code, "SOURCE_INVALID");
        assert!(!error.diagnostics.is_empty());
        for diagnostic in error.diagnostics {
            for location in std::iter::once(&diagnostic.location).chain(&diagnostic.related) {
                assert_eq!(location.address.uri, "memory://raml08/api.raml");
                assert_eq!(
                    location.address.pointer,
                    "/~1items/post/body/application~1json/schema"
                );
                assert_eq!(location.line, Some(9));
            }
        }
    }
    Ok(())
}

#[test]
fn serialized_schema_budgets_bound_distinct_resources_and_allow_reuse()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = json!({"enum":(0..400).collect::<Vec<_>>()}).to_string();
    let quoted = serde_json::to_string(&schema)?;
    let body = format!(
        "    body:\n      application/json:\n        schema: {quoted}\n    responses:\n      204: {{description: Empty}}\n"
    );
    let prefix =
        "#%RAML 0.8\ntitle: Bounded schemas\nversion: v1\nbaseUri: https://api.example.test\n";
    for node_budget in [true, false] {
        let mut input = request(&format!(
            "{prefix}/one:\n  post:\n{body}/two:\n  post:\n{body}"
        ));
        if node_budget {
            input.config.limits.max_nodes = 600;
        } else {
            input.config.limits.max_documents = 1;
        }
        let error = convert(&input).err().ok_or("derived budget exceeded")?;
        assert_eq!(error.code, "SCHEMA_DERIVED_LIMIT");
    }
    let shared_body = body.replace(&format!("schema: {quoted}"), "schema: Shared");
    let mut input = request(&format!(
        "{prefix}schemas:\n  - Shared: {quoted}\n/one:\n  post:\n{shared_body}/two:\n  post:\n{shared_body}"
    ));
    input.config.limits.max_documents = 1;
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    assert_eq!(report.manifest.files.len(), 1);
    Ok(())
}

#[test]
fn legacy_query_string_field_is_rejected_before_openapi_emission()
-> Result<(), Box<dyn std::error::Error>> {
    let input = SOURCE.replace("  get:\n", "  get:\n    queryString: string\n");
    let error = convert(&request(&input))
        .err()
        .ok_or("RAML 1.0 field accepted in 0.8")?;
    assert_eq!(error.code, "RAML_VERSION_FEATURE");
    Ok(())
}

#[test]
fn legacy_form_bodies_reject_schema_even_with_media_parameters()
-> Result<(), Box<dyn std::error::Error>> {
    for media in [
        "multipart/form-data",
        "application/x-www-form-urlencoded",
        "Multipart/Form-Data; boundary=upload",
    ] {
        let source = format!(
            "#%RAML 0.8\ntitle: Forms\nversion: v1\nbaseUri: https://api.example.test\n/items:\n  post:\n    body:\n      {media}:\n        schema: '{{\"type\":\"object\"}}'\n    responses:\n      204: {{description: Empty}}\n"
        );
        let error = convert(&request(&source))
            .err()
            .ok_or("form schema accepted")?;
        assert_eq!(error.code, "RAML_FORM", "{media}");
    }
    Ok(())
}

#[test]
fn legacy_body_examples_keep_unknown_fields_inert_and_literal_example_intact()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "#%RAML 0.8\ntitle: Examples\nversion: v1\nbaseUri: https://api.example.test\n/items:\n  post:\n    body:\n      application/json:\n        example: {value: 7, extra: true}\n        examples:\n          modern: {value: 8}\n    responses:\n      204: {description: Empty}\n";
    let report = convert(&request(source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Complete);
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "RAML_BODY_FACET"
                && decision.source.address.pointer.ends_with("/examples"))
    );
    let output = report.document.ok_or("output")?;
    let media = &output["paths"]["/items"]["post"]["requestBody"]["content"]["application/json"];
    assert!(media.get("examples").is_none());
    assert_eq!(media["example"], json!({"value":7,"extra":true}));
    Ok(())
}

#[test]
fn legacy_oauth_grant_aliases_keep_or_security_and_flow_urls()
-> Result<(), Box<dyn std::error::Error>> {
    let source=SOURCE.replace("      type: Basic Authentication", "      type: OAuth 2.0\n      settings:\n        authorizationUri: https://auth.example.test/authorize\n        accessTokenUri: https://auth.example.test/token\n        authorizationGrants: [code, token]\n        scopes: [read]")
        .replace("securedBy: [basic]", "securedBy: [null, {basic: {scopes: [read]}}]");
    let report = convert(&request(&source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let output = report.document.ok_or("output")?;
    assert_eq!(output["security"], json!([{}, {"basic":["read"]}]));
    let flows = &output["components"]["securitySchemes"]["basic"]["flows"];
    assert_eq!(
        flows["authorizationCode"]["authorizationUrl"],
        "https://auth.example.test/authorize"
    );
    assert_eq!(
        flows["authorizationCode"]["tokenUrl"],
        "https://auth.example.test/token"
    );
    assert_eq!(
        flows["implicit"]["authorizationUrl"],
        "https://auth.example.test/authorize"
    );
    assert!(flows["implicit"].get("tokenUrl").is_none());
    Ok(())
}

#[test]
fn duplicate_declarations_and_invalid_legacy_facets_fail() -> Result<(), Box<dyn std::error::Error>>
{
    for (source, code) in [
        (
            SOURCE.replace(
                "securedBy: [basic]",
                "securedBy: [basic]\ntraits:\n  - a: {}\n  - a: {}",
            ),
            "SOURCE_INVALID",
        ),
        (
            SOURCE.replace("  - paged:\n", "  - paged: {}\n  - paged:\n"),
            "RAML_DECLARATION_DUPLICATE",
        ),
        (
            SOURCE.replace("repeat: true", "repeat: sometimes"),
            "RAML_PARAMETER",
        ),
        (
            SOURCE.replace("required: true", "required: yes"),
            "RAML_PARAMETER",
        ),
        (
            SOURCE.replace("repeat: true", "type: file"),
            "RAML_PARAMETER",
        ),
        (
            SOURCE.replace("repeat: true", "type: [string, integer]"),
            "RAML_PARAMETER",
        ),
    ] {
        let error = convert(&request(&source))
            .err()
            .ok_or("invalid source accepted")?;
        assert_eq!(error.code, code);
    }
    Ok(())
}
