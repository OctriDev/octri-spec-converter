use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::json;

fn request(text: &str, name: &str) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: format!("memory://fixtures/{name}"),
            content: Some(text.into()),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    }
}

#[test]
fn graphql_requires_mapping_and_retains_physical_sdl_location()
-> Result<(), Box<dyn std::error::Error>> {
    let text = "type Query {\n  pet(id: ID!): Pet\n}\ntype Pet { id: ID! name: String }";
    let mut input = request(text, "schema.graphql");
    let blocked = convert(&input)?;
    assert_eq!(blocked.status, Status::Blocked);
    let missing = blocked
        .decisions
        .iter()
        .find(|decision| decision.code == "HTTP_MAPPING_REQUIRED")
        .ok_or("missing blocker")?;
    assert_eq!(missing.source.line, Some(2));
    assert_eq!(
        blocked
            .document
            .as_ref()
            .and_then(|document| document["paths"].as_object())
            .map(|paths| paths.len()),
        Some(0)
    );
    input.config.http_mappings.insert("Query.pet".into(),serde_json::from_value(json!({"method":"GET","path":"/pets/{id}","server":"https://api.example.com","responseStatus":"200","parameterBindings":{"id":{"location":"path","name":"id"}},"security":[],"responseSchema":{"$ref":"#/components/schemas/Pet"}}))?);
    input.config.api_version = Some("1".into());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    assert_eq!(
        report
            .document
            .ok_or("missing output")?
            .pointer("/paths/~1pets~1{id}/get/parameters/0/schema/type"),
        Some(&json!("string"))
    );
    Ok(())
}

#[test]
fn graphql_inputs_keep_defaults_nonnull_lists_and_recursive_types()
-> Result<(), Box<dyn std::error::Error>> {
    let text = "schema { query: Root }\n type Root { pets(filter: Filter!): [Pet!]! }\ninput Filter { limit: Int! = 20 name: String child: Filter }\ntype Pet { id: ID! friend: Pet }\nextend type Pet { age: Int }";
    let mut input = request(text, "schema.graphql");
    input.config.http_mappings.insert("Root.pets".into(),serde_json::from_value(json!({"method":"POST","path":"/pets/search","server":"https://api.example.com","responseStatus":"200","security":[]}))?);
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview);
    let doc = report.document.ok_or("missing output")?;
    assert_eq!(
        doc.pointer("/components/schemas/Filter/properties/limit/default"),
        Some(&json!(20))
    );
    assert!(doc.pointer("/components/schemas/Filter/required").is_none());
    assert!(
        doc.pointer("/components/schemas/Pet/properties/age")
            .is_some()
    );
    assert_eq!(
        doc.pointer(
            "/paths/~1pets~1search/post/requestBody/content/application~1json/schema/required"
        ),
        Some(&json!(["filter"]))
    );
    Ok(())
}

#[test]
fn graphql_unknown_symbols_and_excess_nesting_fail() {
    assert!(convert(&request("type Query { pet: Missing }", "schema.graphql")).is_err());
    let deep = format!(
        "type Query {{ pet: {}ID{} }}",
        "[".repeat(200),
        "]".repeat(200)
    );
    assert!(convert(&request(&deep, "schema.graphql")).is_err());
}

#[test]
fn insomnia_v4_and_v5_keep_group_auth_and_environment() -> Result<(), Box<dyn std::error::Error>> {
    let v4 = json!({"_type":"export","__export_format":4,"resources":[{"_id":"wrk","_type":"workspace","name":"Pets"},{"_id":"env","_type":"environment","parentId":"wrk","data":{"base":"https://api.example.com"}},{"_id":"folder","_type":"request_group","parentId":"wrk","name":"Pet group","authentication":{"type":"bearer","token":"secret"}},{"_id":"request","_type":"request","parentId":"folder","name":"Get pets","method":"GET","url":"{{ _.base }}/pets","authentication":{},"headers":[]}]});
    let v5 = json!({"type":"collection.insomnia.rest/5.0","schema_version":"5.1","name":"Pets","environments":{"data":{"base":"https://api.example.com"}},"collection":[{"name":"Pet group","authentication":{"type":"bearer","token":"secret"},"children":[{"name":"Get pets","method":"GET","url":"{{ _.base }}/pets","headers":[]}]}]});
    for document in [v4, v5] {
        let report = convert(&request(&document.to_string(), "insomnia.json"))?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_eq!(report.status, Status::NeedsReview);
        let doc = report.document.ok_or("missing output")?;
        let operation = doc.pointer("/paths/~1pets/get").ok_or("missing get")?;
        assert_eq!(operation["tags"], json!(["Pet group"]));
        assert!(
            operation["security"]
                .as_array()
                .is_some_and(|requirements| !requirements.is_empty())
        );
        assert_eq!(operation["servers"][0]["url"], "https://api.example.com");
    }
    Ok(())
}

#[test]
fn graphql_introspection_is_not_confused_with_json_schema() -> Result<(), Box<dyn std::error::Error>>
{
    let introspection = json!({"data":{"__schema":{"queryType":{"name":"Query"},"types":[{"kind":"OBJECT","name":"Query","fields":[{"name":"hello","type":{"kind":"SCALAR","name":"String"},"args":[]}]},{"kind":"SCALAR","name":"String"}]}}});
    let report = convert(&request(&introspection.to_string(), "schema.json"))?;
    assert_eq!(report.status, Status::Blocked);
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.message.contains("Query.hello"))
    );
    Ok(())
}
