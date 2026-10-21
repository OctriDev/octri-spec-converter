//! Template expansion must preserve schema bases and authored source evidence.
use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::{Value, json};

fn request(source: &str) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://templates/api.raml".into(),
            content: Some(source.into()),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    }
}

fn request_validator(
    document: &Value,
    path: &str,
) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let mut registry = jsonschema::Registry::new();
    for schema in document["components"]["schemas"]
        .as_object()
        .ok_or("schemas")?
        .values()
    {
        if let Some(id) = schema
            .get("id")
            .or_else(|| schema.get("$id"))
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
    let path = path.replace('~', "~0").replace('/', "~1");
    Ok(jsonschema::options().with_registry(&registry).build(&json!({"$ref":format!("memory://relocated/api.json#/paths/{path}/post/requestBody/content/application~1json/schema")}))?)
}

#[test]
fn traits_keep_included_schema_bases_and_share_unchanged_resources()
-> Result<(), Box<dyn std::error::Error>> {
    for version in ["0.8", "1.0"] {
        let declaration = if version == "0.8" {
            "  - jsonBody:"
        } else {
            "  jsonBody:"
        };
        let schema_field = if version == "0.8" { "schema" } else { "type" };
        let source = format!(
            "#%RAML {version}\ntitle: Included trait\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n{declaration} !include traits/body.raml\n/one:\n  post:\n    is: [jsonBody]\n    responses:\n      204: {{description: Empty}}\n/two:\n  post:\n    is: [jsonBody]\n    responses:\n      204: {{description: Empty}}\n"
        );
        let mut input = request(&source);
        input.files.insert(
            "traits/body.raml".into(),
            format!(
                "body:\n  application/json:\n    {schema_field}: !include schemas/input.json\n"
            ),
        );
        input.files.insert("traits/schemas/input.json".into(),json!({"$schema":"http://json-schema.org/draft-04/schema#","type":"object","properties":{"value":{"$ref":"value.json"}}}).to_string());
        input.files.insert(
            "traits/schemas/value.json".into(),
            json!({"type":"integer"}).to_string(),
        );
        let report = convert(&input)?;
        assert!(report.valid, "{version}: {:?}", report.diagnostics);
        assert_eq!(
            report.status,
            Status::Complete,
            "{version}: {:?}",
            report.decisions
        );
        let document = report.document.ok_or("output")?;
        assert_eq!(
            document["paths"]["/one"]["post"]["requestBody"],
            document["paths"]["/two"]["post"]["requestBody"]
        );
        let schema = request_validator(&document, "/one")?;
        assert!(schema.is_valid(&json!({"value":7})));
        assert!(!schema.is_valid(&json!({"value":"wrong"})));
        assert!(
            report
                .manifest
                .files
                .iter()
                .any(|file| file.uri.ends_with("traits/schemas/value.json"))
        );
        assert!(!serde_json::to_string(&report.manifest.files)?.contains(".octri-schema-"));
    }
    Ok(())
}

#[test]
fn nested_resource_types_and_optional_methods_keep_schema_overrides()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "#%RAML 0.8\ntitle: Types\nversion: v1\nbaseUri: https://api.example.test\nresourceTypes:\n  - base:\n      post?:\n        body:\n          application/json:\n            schema: !include schemas/base.json\n        responses:\n          204: {description: Empty}\n  - child:\n      type: base\n/items:\n  type: child\n  post: {}\n  /custom:\n    type: child\n    post:\n      body:\n        application/json:\n          schema: !include schemas/custom.json\n";
    let mut input = request(source);
    input.files.insert(
        "schemas/base.json".into(),
        json!({"type":"string"}).to_string(),
    );
    input.files.insert(
        "schemas/custom.json".into(),
        json!({"type":"integer"}).to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("output")?;
    let base = request_validator(&document, "/items")?;
    assert!(base.is_valid(&json!("text")));
    assert!(!base.is_valid(&json!(2)));
    let custom = request_validator(&document, "/items/custom")?;
    assert!(custom.is_valid(&json!(2)));
    assert!(!custom.is_valid(&json!("text")));
    let empty_method = source.replace("  post: {}", "  post:");
    let mut input = request(&empty_method);
    input.files.insert(
        "schemas/base.json".into(),
        json!({"type":"string"}).to_string(),
    );
    input.files.insert(
        "schemas/custom.json".into(),
        json!({"type":"integer"}).to_string(),
    );
    let report = convert(&input)?;
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("output")?;
    assert!(request_validator(&document, "/items")?.is_valid(&json!("text")));
    Ok(())
}

#[test]
fn schema_parameter_substitution_has_distinct_derived_resources_per_value()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "#%RAML 0.8\ntitle: Parameterized\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n  - payload:\n      body:\n        application/json:\n          schema:\n            type: <<kind>>\n/text:\n  post:\n    is: [{payload: {kind: string}}]\n    responses:\n      204: {description: Empty}\n/number:\n  post:\n    is: [{payload: {kind: integer}}]\n    responses:\n      204: {description: Empty}\n";
    let report = convert(&request(source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("output")?;
    let text = request_validator(&document, "/text")?;
    assert!(text.is_valid(&json!("text")));
    assert!(!text.is_valid(&json!(2)));
    let number = request_validator(&document, "/number")?;
    assert!(number.is_valid(&json!(2)));
    assert!(!number.is_valid(&json!("text")));
    assert_eq!(report.manifest.files.len(), 1);
    Ok(())
}

#[test]
fn serialized_trait_schema_resolves_relative_refs_from_template_file()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "#%RAML 0.8\ntitle: Serialized\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n  - payload: !include traits/body.raml\n/items:\n  post:\n    is: [payload]\n    responses:\n      204: {description: Empty}\n";
    let mut input = request(source);
    input.files.insert(
        "traits/body.raml".into(),
        "body:\n  application/json:\n    schema: '{\"$ref\":\"schemas/value.json\"}'\n".into(),
    );
    input.files.insert(
        "traits/schemas/value.json".into(),
        json!({"type":"boolean"}).to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    assert!(report.decisions.iter().any(|d| d.code == "RAML_BODY_SCHEMA"
        && d.source.address.uri.ends_with("traits/body.raml")
        && d.source.line == Some(3)));
    Ok(())
}

#[test]
fn expanded_template_errors_keep_physical_scalar_locations()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "#%RAML 0.8\ntitle: Errors\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n  - payload: !include traits/body.raml\n/items:\n  post:\n    is: [payload]\n    responses:\n      204: {description: Empty}\n";
    let mut input = request(source);
    input.files.insert(
        "traits/body.raml".into(),
        "body:\n  application/json:\n    schema: '{\"$ref\":\"#/missing\"}'\n".into(),
    );
    let error = convert(&input).err().ok_or("missing ref accepted")?;
    assert!(
        error
            .diagnostics
            .iter()
            .any(|d| d.location.address.uri.ends_with("traits/body.raml")
                && d.location.address.pointer == "/body/application~1json/schema"
                && d.location.line == Some(3))
    );
    Ok(())
}

#[test]
fn large_repeated_template_parameters_consume_expansion_budget()
-> Result<(), Box<dyn std::error::Error>> {
    let values = serde_json::to_string(&(0..80).collect::<Vec<_>>())?;
    let source = format!(
        "#%RAML 1.0\ntitle: Bounded\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n  payload:\n    queryParameters:\n      one: {{enum: <<values>>}}\n      two: {{enum: <<values>>}}\n      three: {{enum: <<values>>}}\n/items:\n  get:\n    is: [{{payload: {{values: {values}}}}}]\n    responses:\n      204: {{description: Empty}}\n"
    );
    let mut input = request(&source);
    input.config.limits.max_nodes = 200;
    let error = convert(&input).err().ok_or("expansion budget bypassed")?;
    assert_eq!(error.code, "RAML_TEMPLATE_LIMIT");
    Ok(())
}

#[test]
fn included_schema_parameters_keep_their_own_base_through_nested_traits()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "#%RAML 0.8\ntitle: Parameters\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n  - base: !include traits/base.raml\n  - nested:\n      is: [{base: {shape: <<shape>>}}]\n/items:\n  post:\n    is:\n      - nested:\n          shape: !include models/input.json\n    responses:\n      204: {description: Empty}\n";
    let mut input = request(source);
    input.files.insert(
        "traits/base.raml".into(),
        "body:\n  application/json:\n    schema: <<shape>>\n".into(),
    );
    input.files.insert(
        "models/input.json".into(),
        json!({"type":"array","items":{"$ref":"value.json"}}).to_string(),
    );
    input.files.insert(
        "models/value.json".into(),
        json!({"type":"boolean"}).to_string(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("output")?;
    let validator = request_validator(&document, "/items")?;
    assert!(validator.is_valid(&json!([true, false])));
    assert!(!validator.is_valid(&json!([1])));
    assert!(report.decisions.iter().any(
        |d| d.code == "RAML_BODY_SCHEMA" && d.source.address.uri.ends_with("models/input.json")
    ));
    Ok(())
}

#[test]
fn method_like_schema_and_example_properties_remain_literal()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "#%RAML 0.8\ntitle: Literal keys\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n  - payload:\n      body:\n        application/json:\n          schema:\n            type: object\n            properties:\n              get?: {type: string}\n              get: {type: integer}\n            example: {get?: keep, get: 7}\n/items:\n  post:\n    is: [payload]\n    responses:\n      204: {description: Empty}\n";
    let report = convert(&request(source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("output")?;
    let validator = request_validator(&document, "/items")?;
    assert!(validator.is_valid(&json!({"get?":"keep","get":7})));
    assert!(!validator.is_valid(&json!({"get?":8,"get":7})));
    assert!(!validator.is_valid(&json!({"get?":"keep","get":"wrong"})));
    Ok(())
}

#[test]
fn inherited_uri_parameter_constraints_keep_declaration_origins()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "#%RAML 0.8\ntitle: URI origins\nversion: v1\nbaseUri: https://api.example.test\nresourceTypes:\n  - item:\n      uriParameters:\n        id:\n          type: string\n          unsupportedFacet: evidence\n/items/{id}:\n  type: item\n  get:\n    responses:\n      204: {description: Empty}\n  /details:\n    get:\n      responses:\n        204: {description: Empty}\n";
    let report = convert(&request(source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(
        report
            .decisions
            .iter()
            .any(|d| d.code == "RAML_PARAMETER_FACET"
                && d.source.address.pointer
                    == "/resourceTypes/0/item/uriParameters/id/unsupportedFacet"
                && d.source.line == Some(10))
    );
    let document = report.document.ok_or("output")?;
    for path in ["/items/{id}", "/items/{id}/details"] {
        let parameter = &document["paths"][path]["get"]["parameters"][0];
        assert_eq!(parameter["name"], "id");
        assert_eq!(parameter["required"], true);
        assert_eq!(parameter["schema"]["type"], "string");
    }
    Ok(())
}

#[test]
fn physical_schema_addresses_are_not_remapped_through_numeric_aliases_twice()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "#%RAML 0.8\ntitle: Physical schema\nversion: v1\nbaseUri: https://api.example.test\nschemas:\n  - \"0\":\n      type: object\n      properties:\n        type: {type: integer}\n    properties:\n      type: string\n/items:\n  post:\n    body:\n      application/json:\n        schema: properties\n    responses:\n      204: {description: Empty}\n";
    let report = convert(&request(source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("output")?;
    let validator = request_validator(&document, "/items")?;
    assert!(validator.is_valid(&json!("text")));
    assert!(!validator.is_valid(&json!({"type":7})));
    Ok(())
}

#[test]
fn large_resource_type_arrays_merge_without_quadratic_comparison_work()
-> Result<(), Box<dyn std::error::Error>> {
    let count = 20_000;
    let first = serde_json::to_string(&(0..count).collect::<Vec<_>>())?;
    let second = serde_json::to_string(&(count / 2..count + count / 2).collect::<Vec<_>>())?;
    let source = format!(
        "#%RAML 1.0\ntitle: Large merge\nversion: v1\nbaseUri: https://api.example.test\nresourceTypes:\n  values:\n    get:\n      queryParameters:\n        value:\n          type: integer\n          enum: {first}\n/items:\n  type: values\n  get:\n    queryParameters:\n      value:\n        enum: {second}\n    responses:\n      204: {{description: Empty}}\n"
    );
    let report = convert(&request(&source))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("output")?;
    let values = document["paths"]["/items"]["get"]["parameters"][0]["schema"]["enum"]
        .as_array()
        .ok_or("enum")?;
    assert_eq!(values.len(), count + count / 2);
    for (index, value) in values.iter().enumerate() {
        assert_eq!(value, &json!(index));
    }
    Ok(())
}
