#![allow(clippy::unwrap_used, clippy::panic)]

use octri_parser::{
    Input, NoIo, Operation, Options, ReadContext, Request, ResolvedSource, Resolver, Result,
};
use serde_json::json;

fn request(schemas: serde_json::Value) -> Request {
    Request {
        operation: Operation::Analyze,
        input: Input {
            uri: "https://api.example/openapi.json".into(),
            content: Some(
                json!({
                    "openapi":"3.1.2", "info":{"title":"Redirects","version":"1"},
                    "paths":{}, "components":{"schemas":schemas}
                })
                .to_string(),
            ),
            generated: false,
        },
        options: Options::default(),
    }
}

fn rejected_with(input: Request, code: &str) {
    let diagnostics = match octri_parser::execute(input, &mut NoIo) {
        Ok(report) => {
            assert!(!report.valid);
            report.diagnostics
        }
        Err(error) => error.diagnostics,
    };
    assert!(
        diagnostics.iter().any(|diagnostic| diagnostic.code == code),
        "{diagnostics:?}"
    );
}

#[test]
fn virtual_redirects_preserve_final_base_anchors_and_physical_locations() {
    let mut input = request(json!({
        "Pet":{"$ref":"https://old.example/pet.json#Pet"},
        "Name":{"$ref":"https://old.example/middle.json#/properties/a~1b"}
    }));
    input.options.retrieval_aliases.extend([
        (
            "https://old.example/pet.json".into(),
            "https://old.example/middle.json".into(),
        ),
        (
            "https://old.example/middle.json".into(),
            "https://cdn.example/v2/pet.json".into(),
        ),
    ]);
    let original = json!({"$anchor":"Pet", "type":"object", "properties":{
        "a/b":{"$ref":"child.json"}, "node":{"$dynamicRef":"#Pet"}
    }})
    .to_string();
    input.options.files.extend([
        ("https://cdn.example/v2/pet.json".into(), original.clone()),
        (
            "https://cdn.example/v2/child.json".into(),
            json!({"type":"string"}).to_string(),
        ),
    ]);
    let report = octri_parser::execute(input, &mut NoIo).unwrap();
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.sources.len(), 3);
    let source = report
        .sources
        .iter()
        .find(|source| source.uri == "https://cdn.example/v2/pet.json")
        .unwrap();
    assert_eq!(source.text, original);
    assert_eq!(source.document["properties"]["node"]["$dynamicRef"], "#Pet");
    assert!(report.references.iter().any(|edge| {
        edge.target.as_ref().is_some_and(|target| {
            target.address.uri == "https://cdn.example/v2/pet.json"
                && target.address.pointer == "/properties/a~1b"
        })
    }));
    assert!(report.references.iter().any(|edge| {
        edge.target
            .as_ref()
            .is_some_and(|target| target.address.uri == "https://cdn.example/v2/child.json")
    }));
    assert_eq!(
        report.registry.retrieval_aliases["https://old.example/pet.json"],
        "https://cdn.example/v2/pet.json"
    );
}

#[test]
fn relative_schema_ids_use_final_retrieval_uri() {
    let mut input = request(json!({"Pet":{"$ref":"https://old.example/pet.json"}}));
    input.options.retrieval_aliases.insert(
        "https://old.example/pet.json".into(),
        "https://cdn.example/v2/pet.json".into(),
    );
    input.options.files.extend([
        (
            "https://cdn.example/v2/pet.json".into(),
            json!({"$id":"types/pet.json", "$ref":"child.json"}).to_string(),
        ),
        (
            "https://cdn.example/v2/types/child.json".into(),
            json!({"type":"string"}).to_string(),
        ),
    ]);
    let report = octri_parser::execute(input, &mut NoIo).unwrap();
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(
        report
            .registry
            .resources
            .contains_key("https://cdn.example/v2/types/pet.json")
    );
}

#[test]
fn authored_id_equal_to_retrieval_alias_retains_its_anchor_scope() {
    let mut input = request(json!({"Pet":{"$ref":"https://old.example/pet.json#Pet"}}));
    input.options.retrieval_aliases.insert(
        "https://old.example/pet.json".into(),
        "https://cdn.example/v2/pet.json".into(),
    );
    input.options.files.extend([
        ("https://cdn.example/v2/pet.json".into(), json!({"$id":"https://old.example/pet.json", "$anchor":"Pet", "type":"object", "properties":{"name":{"$ref":"child.json"}}}).to_string()),
        ("https://old.example/child.json".into(), json!({"type":"string"}).to_string()),
    ]);
    let report = octri_parser::execute(input, &mut NoIo).unwrap();
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(
        report
            .registry
            .anchors
            .contains_key("https://old.example/pet.json#Pet")
    );
    assert!(
        !report
            .registry
            .anchors
            .contains_key("https://cdn.example/v2/pet.json#Pet")
    );
}

#[test]
fn alias_cycles_fragments_duplicate_keys_and_noncanonical_files_are_rejected() {
    for (aliases, code) in [
        (vec![("old.json", "old.json")], "INVALID_OPTIONS"),
        (
            vec![("old.json", "new.json"), ("new.json", "old.json")],
            "INVALID_OPTIONS",
        ),
        (vec![("old.json#Pet", "new.json")], "INVALID_OPTIONS"),
        (vec![("old.json", "new.json#Pet")], "INVALID_OPTIONS"),
        (
            vec![("old.json", "one.json"), ("./old.json", "two.json")],
            "DUPLICATE_RESOURCE",
        ),
    ] {
        let mut input = request(json!({}));
        input.options.retrieval_aliases = aliases
            .into_iter()
            .map(|(from, to)| (from.into(), to.into()))
            .collect();
        rejected_with(input, code);
    }
    let mut input = request(json!({}));
    input
        .options
        .retrieval_aliases
        .insert("old.json".into(), "new.json".into());
    input.options.files.insert("old.json".into(), "{}".into());
    rejected_with(input, "INVALID_OPTIONS");
}

#[test]
fn authored_resource_cannot_shadow_retrieval_alias() {
    let mut input = request(json!({"Pet":{"$id":"https://old.example/pet.json","type":"string"}}));
    input.options.retrieval_aliases.insert(
        "https://old.example/pet.json".into(),
        "https://cdn.example/pet.json".into(),
    );
    let outcome = octri_parser::execute(input, &mut NoIo);
    assert!(match outcome {
        Ok(report) => report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "DUPLICATE_RESOURCE"),
        Err(error) => error
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "DUPLICATE_RESOURCE"),
    });
}

#[test]
fn retrieval_aliases_have_a_separate_document_budget() {
    let mut input = request(json!({}));
    input.options.limits.max_documents = 1;
    input.options.retrieval_aliases.extend([
        ("one.json".into(), "new.json".into()),
        ("two.json".into(), "new.json".into()),
    ]);
    rejected_with(input, "LIMIT_DOCUMENTS");
}

struct RedirectResolver;
impl Resolver for RedirectResolver {
    fn read(&mut self, _: &str, _: ReadContext<'_>) -> Result<String> {
        unreachable!()
    }
    fn read_with_identity(&mut self, uri: &str, _: ReadContext<'_>) -> Result<ResolvedSource> {
        let (retrieval_uri, document) = if uri == "https://api.example/openapi.json" {
            (
                "https://cdn.example/v2/openapi.json",
                json!({"openapi":"3.1.2","info":{"title":"Redirects","version":"1"},"paths":{},"components":{"schemas":{"Pet":{"$ref":"pet.json"}}}}),
            )
        } else {
            assert_eq!(uri, "https://cdn.example/v2/pet.json");
            (uri, json!({"type":"string"}))
        };
        Ok(ResolvedSource {
            content: document.to_string(),
            retrieval_uri: retrieval_uri.into(),
        })
    }
}

#[test]
fn resolver_final_uri_updates_root_and_subsequent_relative_reads() {
    let mut input = request(json!({}));
    input.input.content = None;
    let report = octri_parser::execute(input, &mut RedirectResolver).unwrap();
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(
        report.registry.root_uri,
        "https://cdn.example/v2/openapi.json"
    );
    assert!(
        report
            .sources
            .iter()
            .any(|source| source.uri == "https://cdn.example/v2/pet.json")
    );
}

#[test]
fn existing_text_only_custom_resolvers_remain_compatible() {
    struct TextResolver;
    impl Resolver for TextResolver {
        fn read(&mut self, _: &str, _: ReadContext<'_>) -> Result<String> {
            Ok(json!({"type":"string"}).to_string())
        }
    }
    let report = octri_parser::execute(
        request(json!({"Pet":{"$ref":"pet.json"}})),
        &mut TextResolver,
    )
    .unwrap();
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(report.registry.retrieval_aliases.is_empty());
}

#[test]
fn multiple_native_aliases_reuse_identical_sources_and_reject_conflicting_bytes() {
    struct SharedFinal {
        conflicting: bool,
    }
    impl Resolver for SharedFinal {
        fn read(&mut self, _: &str, _: ReadContext<'_>) -> Result<String> {
            unreachable!()
        }
        fn read_with_identity(&mut self, uri: &str, _: ReadContext<'_>) -> Result<ResolvedSource> {
            Ok(ResolvedSource {
                retrieval_uri: "https://cdn.example/pet.json".into(),
                content: json!({"type": if self.conflicting && uri.ends_with("b.json") { "integer" } else { "string" }}).to_string(),
            })
        }
    }
    for conflicting in [false, true] {
        let input = request(json!({"A":{"$ref":"a.json"},"B":{"$ref":"b.json"}}));
        let report = octri_parser::execute(input, &mut SharedFinal { conflicting }).unwrap();
        assert_eq!(report.valid, !conflicting, "{:?}", report.diagnostics);
        assert_eq!(report.sources.len(), 2);
        if conflicting {
            assert!(
                report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "DUPLICATE_RESOURCE")
            );
        }
    }
}

#[test]
fn aliases_do_not_grant_io_and_bundle_remains_portable() {
    let mut input = request(json!({"Pet":{"$ref":"old.json"}}));
    input
        .options
        .retrieval_aliases
        .insert("old.json".into(), "https://cdn.example/pet.json".into());
    let missing = octri_parser::execute(input.clone(), &mut NoIo).unwrap();
    assert!(!missing.valid);
    assert!(
        missing
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RESOLVER_DENIED"
                && diagnostic.location.address.uri == "https://cdn.example/pet.json")
    );

    input.options.files.insert(
        "https://cdn.example/pet.json".into(),
        json!({"type":"string"}).to_string(),
    );
    input.operation = Operation::Bundle;
    let bundled = octri_parser::execute(input, &mut NoIo).unwrap();
    assert!(bundled.valid, "{:?}", bundled.diagnostics);
    let mut moved = request(json!({}));
    moved.input.uri = "memory://moved/openapi.json".into();
    moved.input.content = Some(bundled.document.unwrap().to_string());
    let checked = octri_parser::execute(moved, &mut NoIo).unwrap();
    assert!(checked.valid, "{:?}", checked.diagnostics);
}
