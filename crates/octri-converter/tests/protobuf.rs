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
fn compiles_virtual_imports_and_preserves_protojson_types() -> Result<(), Box<dyn std::error::Error>>
{
    let source = "syntax = \"proto3\"; package pets; import \"pet.proto\"; message GetRequest { string id = 1; } service Pets { rpc Get(GetRequest) returns (Pet); }";
    let mut input = request(source, "main.proto");
    input.files.insert("pet.proto".into(),"syntax = \"proto3\"; package pets; message Pet { uint64 id = 1; repeated string tags = 2; map<string, int32> counts = 3; oneof choice { string name = 4; bool anonymous = 5; } }".into());
    let blocked = convert(&input)?;
    assert_eq!(blocked.status, Status::Blocked);
    assert!(
        blocked
            .decisions
            .iter()
            .any(|decision| decision.message.contains("pets.Pets.Get"))
    );
    input.config.http_mappings.insert("pets.Pets.Get".into(),serde_json::from_value(json!({"method":"GET","path":"/pets/{id}","server":"https://api.example.com","responseStatus":"200","parameterBindings":{"id":{"location":"path","name":"id"}},"security":[]}))?);
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview);
    let doc = report.document.ok_or("missing output")?;
    assert_eq!(
        doc.pointer("/components/schemas/pets.Pet/properties/id/anyOf/0/type"),
        Some(&json!("string"))
    );
    assert_eq!(
        doc.pointer("/components/schemas/pets.Pet/properties/tags/anyOf/0/items/type"),
        Some(&json!("string"))
    );
    assert_eq!(
        doc.pointer(
            "/components/schemas/pets.Pet/properties/counts/anyOf/0/additionalProperties/type"
        ),
        Some(&json!("integer"))
    );
    assert!(
        doc.pointer("/components/schemas/pets.Pet/allOf/0/oneOf")
            .is_some()
    );
    Ok(())
}

#[test]
fn missing_imports_and_streaming_never_claim_unary_success()
-> Result<(), Box<dyn std::error::Error>> {
    assert!(
        convert(&request(
            "syntax = \"proto3\"; import \"/etc/passwd\"; message Test {}",
            "test.proto"
        ))
        .is_err()
    );
    let source = "syntax = \"proto3\"; message Empty {} service Events { rpc Watch(Empty) returns (stream Empty); }";
    let mut input = request(source, "stream.proto");
    input.config.http_mappings.insert("Events.Watch".into(),serde_json::from_value(json!({"method":"GET","path":"/events","server":"https://api.example.com","responseStatus":"200","security":[]}))?);
    let report = convert(&input)?;
    assert_eq!(report.status, Status::Blocked);
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "GRPC_STREAMING_MAPPING")
    );
    Ok(())
}
