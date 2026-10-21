//! Serialized RAML 1.0 schemas share reference scope and evidence with 0.8.
use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::{Value, json};

fn request(schema: &str) -> ConversionRequest {
    let declaration = json!(schema).to_string();
    ConversionRequest {
        input: Input { uri:"memory://raml/api.raml".into(),generated:false,content:Some("#%RAML 1.0\ntitle: Serialized schemas\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n  payload: !include traits/body.raml\n/items:\n  post:\n    is: [payload]\n    responses:\n      204: {description: Empty}\n".into()) },
        files: [("traits/body.raml".into(),format!("body:\n  application/json:\n    type: {declaration}\n"))].into(),
        config:Config::default()
    }
}

fn validator(document: &Value) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
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
            "memory://relocated/api.json",
            jsonschema::Resource::from_contents(document.clone()),
        )?
        .prepare()?;
    Ok(jsonschema::options().with_registry(&registry).build(&json!({"$ref":"memory://relocated/api.json#/paths/~1items/post/requestBody/content/application~1json/schema"}))?)
}

#[test]
fn template_schema_text_keeps_its_relative_base_and_declared_dialect()
-> Result<(), Box<dyn std::error::Error>> {
    for (dialect, definitions) in [
        ("http://json-schema.org/draft-04/schema#", "definitions"),
        ("http://json-schema.org/draft-07/schema#", "definitions"),
        ("https://json-schema.org/draft/2020-12/schema", "$defs"),
    ] {
        let schema = json!({"$schema":dialect,"type":"object","properties":{"value":{"$ref":format!("#/{definitions}/Value")}},"required":["value"],definitions:{"Value":{"$ref":"value.json"}}});
        for bom in ["", "\u{feff}"] {
            let mut input = request(&format!("{bom}{schema}"));
            input.files.insert(
                "traits/value.json".into(),
                json!({"type":"integer","minimum":1}).to_string(),
            );
            let report = convert(&input)?;
            assert!(report.valid, "{:?}", report.diagnostics);
            assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
            assert_eq!(report.manifest.files.len(), 3);
            assert!(!serde_json::to_string(&report.manifest.files)?.contains(".octri-schema-"));
            let check = validator(&report.document.ok_or("output")?)?;
            assert!(check.is_valid(&json!({"value":1})));
            assert!(!check.is_valid(&json!({"value":0})));
            assert!(!check.is_valid(&json!({"value":"wrong"})));
        }
    }
    Ok(())
}

#[test]
fn syntax_and_reference_errors_identify_original_template_scalar()
-> Result<(), Box<dyn std::error::Error>> {
    for (schema, facet) in [
        ("{", "type"),
        ("{\"$ref\":\"#/missing\"}", "type"),
        ("{", "schema"),
        ("{\"$ref\":\"#/missing\"}", "schema"),
    ] {
        let mut input = request(schema);
        let body = input.files.get_mut("traits/body.raml").ok_or("body")?;
        *body = body.replace("type:", &format!("{facet}:"));
        let error = convert(&input).err().ok_or("invalid schema accepted")?;
        assert_eq!(error.code, "SOURCE_INVALID");
        assert!(!error.diagnostics.is_empty());
        for diagnostic in &error.diagnostics {
            assert_eq!(
                diagnostic.location.address.uri,
                "memory://raml/traits/body.raml"
            );
            assert_eq!(
                diagnostic.location.address.pointer,
                format!("/body/application~1json/{facet}")
            );
            assert_eq!(diagnostic.location.line, Some(3));
        }
    }
    Ok(())
}

#[test]
fn schema_ids_and_literal_data_keep_separate_scopes() -> Result<(), Box<dyn std::error::Error>> {
    let schema = json!({"$id":"https://schemas.example.test/input.json","type":"object","properties":{"value":{"$ref":"value.json"}},"x-evidence":{"$ref":"inert.json"}});
    let mut input = request(&schema.to_string());
    input.files.insert(
        "https://schemas.example.test/value.json".into(),
        json!({"type":"boolean"}).to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let check = validator(&report.document.ok_or("output")?)?;
    assert!(check.is_valid(&json!({"value":true})));
    assert!(!check.is_valid(&json!({"value":"wrong"})));
    Ok(())
}

#[test]
fn library_schema_text_is_not_rewritten_as_a_type_expression()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = json!({"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"Code":{"const":"Code"},"value":{"$ref":"value.json"}},"required":["Code","value"]});
    let quoted = json!(schema.to_string()).to_string();
    let mut input = request("{}");
    input.input.content=Some("#%RAML 1.0\ntitle: Libraries\nversion: v1\nbaseUri: https://api.example.test\nuses:\n  contracts: lib/contracts.raml\n/items:\n  post:\n    is: [contracts.payload]\n    responses:\n      204: {description: Empty}\n".into());
    input.files.clear();
    input.files.insert("lib/contracts.raml".into(),format!("#%RAML 1.0 Library\ntypes:\n  Code: string\ntraits:\n  payload:\n    body:\n      application/json:\n        type: {quoted}\n"));
    input.files.insert(
        "lib/value.json".into(),
        json!({"type":"boolean"}).to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let check = validator(&report.document.ok_or("output")?)?;
    assert!(check.is_valid(&json!({"Code":"Code","value":true})));
    assert!(!check.is_valid(&json!({"Code":"contracts.Code","value":true})));
    assert!(!check.is_valid(&json!({"Code":"Code","value":"wrong"})));
    Ok(())
}

#[test]
fn schema_documents_are_forbidden_in_parameters_properties_and_expressions()
-> Result<(), Box<dyn std::error::Error>> {
    let schema =
        json!({"$schema":"http://json-schema.org/draft-07/schema#","type":"string","minLength":2})
            .to_string();
    let quoted = json!(schema).to_string();
    let prefix = "#%RAML 1.0\ntitle: Placement\nversion: v1\nbaseUri: https://api.example.test\n";
    let declarations = [
        format!("baseUriParameters:\n  tenant:\n    type: {quoted}\n"),
        format!("/items:\n  get:\n    queryParameters:\n      q:\n        type: {quoted}\n"),
        format!("/items:\n  get:\n    queryString:\n      type: {quoted}\n"),
        format!("/items:\n  get:\n    headers:\n      X-Code:\n        type: {quoted}\n"),
        format!("/items/{{id}}:\n  uriParameters:\n    id:\n      type: {quoted}\n  get: {{}}\n"),
        format!(
            "/items:\n  get:\n    responses:\n      200:\n        headers:\n          X-Code:\n            type: {quoted}\n"
        ),
        format!(
            "types:\n  Raw: {quoted}\n  Alias: ' Raw '\n  Wrapped:\n    type: ' Alias '\n    description: Schema alias\n/items:\n  get:\n    queryParameters:\n      q: ' Wrapped '\n"
        ),
        format!("types:\n  Raw: {quoted}\n  Object:\n    properties:\n      value: Raw\n"),
        format!("types:\n  Raw: {quoted}\n  Array: Raw[]\n"),
        format!("types:\n  Raw: {quoted}\n  Union: Raw | string\n"),
        format!("types:\n  Raw: {quoted}\n  Multiple:\n    type: [Raw, string]\n"),
    ];
    for declaration in declarations {
        for facet in ["type", "schema"] {
            let declaration = declaration.replace("type:", &format!("{facet}:"));
            let mut input = request("{}");
            input.input.content = Some(format!("{prefix}{declaration}"));
            input.files.clear();
            let error = convert(&input)
                .err()
                .ok_or_else(|| format!("Invalid placement accepted: {declaration}"))?;
            assert_eq!(
                error.code, "RAML_SCHEMA_PLACEMENT",
                "{declaration}: {error:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn deprecated_schema_aliases_preserve_constraints_and_library_origins()
-> Result<(), Box<dyn std::error::Error>> {
    let schema =
        json!({"$schema":"http://json-schema.org/draft-07/schema#","type":"string","minLength":3});
    let quoted = json!(schema.to_string()).to_string();
    for library in [false, true] {
        let mut input = request("{}");
        let (group, body) = if library {
            input.files.clear();
            input.files.insert("lib/contracts.raml".into(),format!("#%RAML 1.0 Library\nschemas:\n  Raw:\n    schema: {quoted}\n    description: Declared schema\n"));
            (
                "uses:\n  contracts: lib/contracts.raml\n".to_owned(),
                "contracts.Raw",
            )
        } else {
            input.files.clear();
            (
                format!(
                    "schemas:\n  Raw:\n    schema: {quoted}\n    description: Declared schema\n"
                ),
                "Raw",
            )
        };
        input.input.content = Some(format!(
            "#%RAML 1.0\ntitle: Aliases\nversion: v1\nbaseUri: https://api.example.test\n{group}/items:\n  post:\n    body:\n      application/json:\n        schema: {body}\n    responses:\n      204: {{description: Empty}}\n"
        ));
        let report = convert(&input)?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
        let check = validator(&report.document.ok_or("output")?)?;
        assert!(check.is_valid(&json!("abc")));
        assert!(!check.is_valid(&json!("ab")));
        assert!(!check.is_valid(&json!(123)));
    }
    Ok(())
}

#[test]
fn alias_collisions_fail_without_rewriting_literal_schema_data()
-> Result<(), Box<dyn std::error::Error>> {
    for declaration in [
        "types: {}\nschemas: {}\n",
        "types:\n  Value: {type: string, schema: string}\n",
        "/items:\n  post:\n    body:\n      application/json: {type: string, schema: string}\n",
    ] {
        let mut input = request("{}");
        input.input.content = Some(format!("#%RAML 1.0\ntitle: Collision\n{declaration}"));
        input.files.clear();
        assert_eq!(
            convert(&input)
                .err()
                .ok_or("alias collision accepted")?
                .code,
            "RAML_TYPE_ALIAS"
        );
    }
    let schema = json!({"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"schema":{"type":"string"},"type":{"type":"string"}},"required":["schema","type"]});
    let report = convert(&request(&schema.to_string()))?;
    let check = validator(&report.document.ok_or("output")?)?;
    assert!(check.is_valid(&json!({"schema":"literal","type":"literal"})));
    assert!(!check.is_valid(&json!({"schema":true,"type":"literal"})));
    Ok(())
}

#[test]
fn schema_body_media_and_wrappers_enforce_raml_contract() -> Result<(), Box<dyn std::error::Error>>
{
    let schema =
        json!({"$schema":"http://json-schema.org/draft-07/schema#","type":"string"}).to_string();
    for media in ["application/xml", "text/plain", "application/octet-stream"] {
        let mut input = request(&schema);
        let body = input.files.get_mut("traits/body.raml").ok_or("body")?;
        *body = body.replace("application/json", media);
        assert_eq!(
            convert(&input)
                .err()
                .ok_or("incompatible media accepted")?
                .code,
            "RAML_SCHEMA_PLACEMENT"
        );
    }
    for media in [
        "application/problem+json",
        "Application/JSON; charset=utf-8",
        "text/json",
    ] {
        let mut input = request(&schema);
        let body = input.files.get_mut("traits/body.raml").ok_or("body")?;
        *body = body.replace("application/json", media);
        let report = convert(&input)?;
        assert!(report.valid, "{media}: {:?}", report.diagnostics);
    }
    let quoted = json!(schema).to_string();
    for facet in [
        "properties: {extra: string}",
        "minLength: 2",
        "default: example",
        "enum: [example]",
    ] {
        for alias in ["type", "schema"] {
            let mut input = request("{}");
            input.input.content = Some(format!(
                "#%RAML 1.0\ntitle: Specialization\nversion: v1\nbaseUri: https://api.example.test\ntypes:\n  Raw: {quoted}\n  Wrapped:\n    {alias}: ' Raw '\n    {facet}\n"
            ));
            input.files.clear();
            assert_eq!(
                convert(&input)
                    .err()
                    .ok_or("schema specialization accepted")?
                    .code,
                "RAML_SCHEMA_SPECIALIZATION"
            );
        }
    }
    Ok(())
}
