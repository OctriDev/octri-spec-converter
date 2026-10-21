use octri_converter::{ConversionRequest, convert};
use serde_json::json;

fn request(content: &str) -> Result<ConversionRequest, serde_json::Error> {
    serde_json::from_value(
        json!({"input":{"uri":"memory://graphql/schema.graphql","content":content}}),
    )
}

#[test]
fn syntactically_valid_but_semantically_invalid_type_kinds_never_project()
-> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "type Query { pet(input: Pet): String } type Pet { id: ID }",
        "type Query { pet: Filter } input Filter { id: ID }",
        "type Query { value: String } union Result = String",
        "type Query implements Missing { value: String }",
        "type Query { value(input: Loop): String } input Loop { child: Loop! }",
        "type Query { value(input: A): String } input A { b: B! } input B { a: A! }",
        "type Query { value(input: String, input: Int): String }",
        "schema { query: Query mutation: Query } type Query { value: String }",
    ] {
        let error = convert(&request(source)?)
            .err()
            .ok_or("invalid GraphQL accepted")?;
        assert_eq!(error.code, "GRAPHQL_SEMANTICS", "{source}: {error:?}");
    }
    Ok(())
}

#[test]
fn nullable_or_list_edges_break_required_input_cycles() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "type Query { value(input: Loop): String } input Loop { child: Loop }",
        "type Query { value(input: Loop): String } input Loop { children: [Loop!]! }",
    ] {
        let report = convert(&request(source)?)?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert!(
            !report.handoff_allowed,
            "Explicit HTTP mapping remains required"
        );
    }
    Ok(())
}
