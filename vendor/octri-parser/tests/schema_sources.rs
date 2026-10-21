#![allow(clippy::expect_used)]

use octri_parser::{
    Address, Input, NoIo, Operation, Options, Request, ValidationMode,
    bundle_schema_registry_sources, bundle_schema_sources, execute,
};
use serde_json::{Value, json};

fn envelope(reference: &str) -> Input {
    Input {
        uri: "memory://generated/openapi.json".into(),
        content: Some(json!({"openapi":"3.1.2","info":{"title":"Imported","version":"1"},"paths":{},"components":{"schemas":{"Payload":{"$ref":reference}}}}).to_string()),
        generated: true,
    }
}

fn root(uri: &str, pointer: &str) -> Address {
    Address {
        uri: uri.into(),
        pointer: pointer.into(),
    }
}

#[test]
fn redirected_registry_keeps_unused_roots_inactive() -> Result<(), Box<dyn std::error::Error>> {
    let requested = "https://old.example/asyncapi.json";
    let final_uri = "https://cdn.example/v2/asyncapi.json";
    let mut options = Options::default();
    options
        .retrieval_aliases
        .insert(requested.into(), final_uri.into());
    options.files.insert(
        final_uri.into(),
        json!({
            "asyncapi":"3.0.0",
            "payload":{"type":"string"},
            "components":{"schemas":{"Unused":{"$ref":"https://never-read.invalid/schema"}}}
        })
        .to_string(),
    );
    let bundle = bundle_schema_registry_sources(
        envelope(&format!("{requested}#/payload")),
        vec![root(requested, "/payload")],
        vec![root(requested, "/components/schemas/Unused")],
        options,
        &mut NoIo,
    )?;
    assert!(!bundle.document.to_string().contains("never-read"));
    assert!(
        bundle
            .origins
            .values()
            .any(|origin| origin.uri == final_uri && origin.pointer == "/payload")
    );
    reparse(bundle.document)
}

#[test]
fn available_registry_resolves_only_selected_transitive_resources()
-> Result<(), Box<dyn std::error::Error>> {
    let uri = "memory://source/asyncapi.json";
    let mut options = Options::default();
    options.files.insert(
        uri.into(),
        json!({
            "asyncapi":"3.0.0",
            "payload":{"$ref":"https://schemas.example/tree#node"},
            "components":{"schemas":{
                "Tree":{"$id":"https://schemas.example/tree", "$anchor":"node", "type":"object",
                    "properties":{"label":{"$ref":"label#label"},"next":{"$ref":"#node"}}},
                "Label":{"$id":"https://schemas.example/label", "$anchor":"label", "type":"string"},
                "Unused":{"$ref":"https://never-read.invalid/schema"},
                "TreeUnused":{"$ref":"https://also-never-read.invalid/schema"}
            }}
        })
        .to_string(),
    );
    let bundle = bundle_schema_registry_sources(
        envelope(&format!("{uri}#/payload")),
        vec![root(uri, "/payload")],
        ["Tree", "Label", "Unused", "TreeUnused"]
            .into_iter()
            .map(|name| root(uri, &format!("/components/schemas/{name}")))
            .collect(),
        options,
        &mut NoIo,
    )?;
    assert!(!bundle.document.to_string().contains("never-read"));
    assert!(
        bundle
            .origins
            .values()
            .any(|origin| origin.pointer == "/components/schemas/Label")
    );
    assert!(
        !bundle
            .origins
            .values()
            .any(|origin| origin.pointer.starts_with("/components/schemas/Unused"))
    );
    reparse(bundle.document)
}

#[test]
fn selected_registry_fragments_keep_enclosing_resource_scope()
-> Result<(), Box<dyn std::error::Error>> {
    let uri = "memory://source/asyncapi.json";
    let mut options = Options::default();
    options.files.insert(
        uri.into(),
        json!({
            "components":{"schemas":{
                "Resource":{"$id":"https://schemas.example/resource", "$defs":{
                    "Selected":{"$ref":"#/$defs/Value"},"Value":{"type":"integer"}
                }},
                "ResourceOther":{"$ref":"https://never-read.invalid/schema"}
            }}
        })
        .to_string(),
    );
    let bundle = bundle_schema_registry_sources(
        envelope(&format!(
            "{uri}#/components/schemas/Resource/$defs/Selected"
        )),
        vec![root(uri, "/components/schemas/Resource/$defs/Selected")],
        vec![
            root(uri, "/components/schemas/Resource"),
            root(uri, "/components/schemas/ResourceOther"),
        ],
        options,
        &mut NoIo,
    )?;
    assert!(!bundle.document.to_string().contains("never-read"));
    reparse(bundle.document)
}

fn reparse(document: Value) -> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options::default();
    options.validate.mode = ValidationMode::Strict;
    let report = execute(
        Request {
            operation: Operation::Analyze,
            input: Input {
                uri: "memory://relocated/api.json".into(),
                content: Some(document.to_string()),
                generated: true,
            },
            options,
        },
        &mut NoIo,
    )?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(report.references.iter().all(|edge| edge.resolved));
    Ok(())
}

#[test]
fn foreign_roots_keep_shared_resource_anchors_dynamic_scope_and_origins()
-> Result<(), Box<dyn std::error::Error>> {
    let uri = "memory://source/asyncapi.json";
    let mut options = Options::default();
    options.files.insert(
        uri.into(),
        json!({
            "asyncapi":"3.0.0",
            "examples":{"$ref":"never-read.json"},
            "components":{"schemas":{
                "Tree":{"$dynamicAnchor":"node","type":"object","properties":{
                    "child":{"$dynamicRef":"#node"},
                    "other":{"$ref":"#/components/schemas/Other"}
                },"example":{"$ref":"also-inert.json"}},
                "Other":{"$id":"https://schemas.example/other","$anchor":"other","type":"string"}
            }}
        })
        .to_string(),
    );
    let bundle = bundle_schema_sources(
        envelope(&format!("{uri}#/components/schemas/Tree")),
        vec![
            root(uri, "/components/schemas/Tree"),
            root(uri, "/components/schemas/Other"),
        ],
        options,
        &mut NoIo,
    )?;
    let wrapper = &bundle.document["components"]["schemas"]["OctriExternal1"];
    assert_eq!(wrapper["$id"], uri);
    let origins: Vec<_> = bundle
        .origins
        .iter()
        .filter(|(_, origin)| origin.pointer == "/components/schemas/Tree/properties/child")
        .collect();
    assert_eq!(origins.len(), 1);
    let path = origins[0].0;
    assert_eq!(
        bundle
            .document
            .pointer(path)
            .and_then(|value| value["$dynamicRef"].as_str()),
        Some("memory://source/asyncapi.json#node")
    );
    assert_eq!(bundle.sources.len(), 2);
    reparse(bundle.document)
}

#[test]
fn schema_targets_are_discovered_without_activating_foreign_reference_objects()
-> Result<(), Box<dyn std::error::Error>> {
    let uri = "memory://source/asyncapi.json";
    let mut options = Options::default();
    options.files.insert(uri.into(), json!({"asyncapi":"3.0.0","ignored":{"$ref":"missing.json"},"payload":{"$ref":"#/types/Next"},"types":{"Next":{"type":"object","properties":{"self":{"$ref":"#/payload"}}}}}).to_string());
    let bundle = bundle_schema_sources(
        envelope(&format!("{uri}#/payload")),
        vec![root(uri, "/payload")],
        options,
        &mut NoIo,
    )?;
    assert!(
        bundle
            .origins
            .values()
            .any(|origin| origin.pointer == "/types/Next/properties/self")
    );
    reparse(bundle.document)
}

#[test]
fn embedded_ids_resolve_without_fetching_logical_identifiers()
-> Result<(), Box<dyn std::error::Error>> {
    let uri = "memory://source/schema.json";
    let mut options = Options::default();
    options.files.insert(uri.into(), json!({"$id":"https://schemas.example/root","$defs":{"child":{"$id":"child","$anchor":"value","type":"integer"}},"$ref":"child#value"}).to_string());
    let bundle = bundle_schema_sources(envelope(uri), vec![root(uri, "")], options, &mut NoIo)?;
    reparse(bundle.document)
}

#[test]
fn cross_document_schema_cycles_remain_bounded_and_portable()
-> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options::default();
    options.files.insert(
        "memory://source/a.json".into(),
        json!({"type":"object","properties":{"b":{"$ref":"b.json"}}}).to_string(),
    );
    options.files.insert(
        "memory://source/b.json".into(),
        json!({"type":"object","properties":{"a":{"$ref":"a.json"}}}).to_string(),
    );
    let bundle = bundle_schema_sources(
        envelope("memory://source/a.json"),
        vec![root("memory://source/a.json", "")],
        options,
        &mut NoIo,
    )?;
    assert_eq!(bundle.sources.len(), 3);
    reparse(bundle.document)
}

#[test]
fn duplicate_ids_and_unsupplied_resources_fail_closed() {
    let uri = "memory://source/foreign.json";
    let mut options = Options::default();
    options.files.insert(uri.into(), json!({"a":{"$id":"https://schemas.example/duplicate"},"b":{"$id":"https://schemas.example/duplicate"}}).to_string());
    let error = bundle_schema_sources(
        envelope(&format!("{uri}#/a")),
        vec![root(uri, "/a"), root(uri, "/b")],
        options,
        &mut NoIo,
    )
    .expect_err("duplicate IDs must fail");
    assert!(
        error
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "DUPLICATE_RESOURCE")
    );
    let error = bundle_schema_sources(
        envelope("https://unprovided.example/schema.json"),
        vec![root("https://unprovided.example/schema.json", "")],
        Options::default(),
        &mut NoIo,
    )
    .expect_err("NoIo must reject retrieval");
    assert!(
        error
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RESOLVER_DENIED")
    );
}

#[test]
fn dynamic_scope_preserves_recursive_instance_validation() -> Result<(), Box<dyn std::error::Error>>
{
    let tree = json!({"$schema":"https://json-schema.org/draft/2020-12/schema","$id":"https://schemas.example/tree","$dynamicAnchor":"node","type":"object","properties":{"data":true,"children":{"type":"array","items":{"$dynamicRef":"#node"}}}});
    let strict = json!({"$schema":"https://json-schema.org/draft/2020-12/schema","$id":"https://schemas.example/strict-tree","$dynamicAnchor":"node","$ref":"tree","unevaluatedProperties":false});
    let original_registry = jsonschema::Registry::new()
        .add(
            "https://schemas.example/tree",
            jsonschema::Resource::from_contents(tree.clone()),
        )?
        .prepare()?;
    let original = jsonschema::options()
        .with_registry(&original_registry)
        .build(&strict)?;
    let uri = "memory://foreign/asyncapi.json";
    let mut options = Options::default();
    options.files.insert(
        uri.into(),
        json!({"asyncapi":"3.0.0","schemas":{"Tree":tree,"Strict":strict}}).to_string(),
    );
    let bundle = bundle_schema_sources(
        envelope(&format!("{uri}#/schemas/Strict")),
        vec![root(uri, "/schemas/Tree"), root(uri, "/schemas/Strict")],
        options,
        &mut NoIo,
    )?;
    let definitions = bundle
        .document
        .pointer("/components/schemas/OctriExternal1/$defs")
        .and_then(Value::as_object)
        .ok_or("missing imported schemas")?;
    let bundled_tree = definitions
        .values()
        .find(|schema| schema["$id"] == "https://schemas.example/tree")
        .ok_or("missing tree resource")?;
    let bundled_strict = definitions
        .values()
        .find(|schema| schema["$id"] == "https://schemas.example/strict-tree")
        .ok_or("missing strict resource")?;
    let bundled_registry = jsonschema::Registry::new()
        .add(
            "https://schemas.example/tree",
            jsonschema::Resource::from_contents(bundled_tree.clone()),
        )?
        .prepare()?;
    let converted = jsonschema::options()
        .with_registry(&bundled_registry)
        .build(bundled_strict)?;
    for (instance, valid) in [
        (json!({"data":1,"children":[{"data":2}]}), true),
        (json!({"data":1,"unexpected":true}), false),
        (json!({"children":[{"unexpected":true}]}), false),
    ] {
        assert_eq!(original.is_valid(&instance), valid, "original: {instance}");
        assert_eq!(converted.is_valid(&instance), valid, "bundled: {instance}");
    }
    reparse(bundle.document)
}
