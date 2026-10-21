use octri_parser::{
    Address, Input, NoIo, Operation, Options, Request, SchemaDialect, SchemaRoot, ValidationMode,
    bundle_schema_registry_with_dialects, execute,
};
use serde_json::{Value, json};

const SOURCE: &str = "memory://events/asyncapi.json";

fn root(pointer: &str, dialect: SchemaDialect) -> SchemaRoot {
    SchemaRoot {
        address: Address {
            uri: SOURCE.into(),
            pointer: pointer.into(),
        },
        dialect,
    }
}

fn bundle(
    source: Value,
    registry: Vec<SchemaRoot>,
) -> Result<octri_parser::BundledSource, octri_parser::ParserError> {
    let mut options = Options::default();
    options.files.insert(SOURCE.into(), source.to_string());
    bundle_schema_registry_with_dialects(
        Input {
            uri: "memory://generated/api.json".into(),
            content: Some(json!({"openapi":"3.1.2","info":{"title":"Events","version":"1"},"paths":{},"components":{"schemas":{"Payload":{"$ref":format!("{SOURCE}#/payload")}}}}).to_string()),
            generated: true,
        },
        vec![root("/payload", SchemaDialect::Draft7)], registry,
        options, &mut NoIo,
    )
}

fn analyze(document: &Value) -> Result<octri_parser::Report, octri_parser::ParserError> {
    let mut options = Options::default();
    options.validate.mode = ValidationMode::Strict;
    execute(
        Request {
            operation: Operation::Analyze,
            input: Input {
                uri: "memory://relocated/openapi.json".into(),
                content: Some(document.to_string()),
                generated: true,
            },
            options,
        },
        &mut NoIo,
    )
}

fn validator(document: &Value) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let mut registry = jsonschema::Registry::new();
    for schema in document
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .ok_or("schemas")?
        .values()
    {
        if let Some(id) = schema.get("$id").and_then(Value::as_str) {
            registry = registry.add(id, jsonschema::Resource::from_contents(schema.clone()))?;
        }
    }
    Ok(jsonschema::options()
        .with_registry(&registry.prepare()?)
        .build(&document["components"]["schemas"]["Payload"])?)
}

#[test]
fn foreign_draft7_preserves_tuple_dependencies_and_fragment_identifiers()
-> Result<(), Box<dyn std::error::Error>> {
    let source = json!({
        "asyncapi":"3.0.0",
        "payload":{
            "type":"object",
            "properties":{
                "code":{"$ref":"#Code"},
                "pair":{"type":"array","items":[{"$ref":"#Code"},{"type":"integer"}],"additionalItems":false}
            },
            "dependencies":{"card":["billing"],"flag":{"properties":{"code":{"$ref":"#Code"}},"required":["code"]}},
            "additionalItems":false,
            "$defs":{"literal":false},
            "example":{"$ref":"literal.json"}
        },
        "components":{"schemas":{"Code":{"$id":"#Code","type":"string","minLength":2}}}
    });
    let result = bundle(
        source.clone(),
        vec![root("/components/schemas/Code", SchemaDialect::Draft7)],
    )?;
    assert_eq!(
        result
            .sources
            .iter()
            .find(|document| document.uri == SOURCE)
            .ok_or("source")?
            .document,
        source
    );
    assert!(
        result
            .origins
            .values()
            .any(|address| address.pointer == "/payload/dependencies/flag/properties/code")
    );
    let report = analyze(&result.document)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(report.references.iter().all(|reference| reference.resolved));
    let validator = validator(&result.document)?;
    for instance in [
        json!({}),
        json!({"pair":[]}),
        json!({"pair":["ab",2]}),
        json!({"card":1,"billing":1}),
        json!({"flag":true,"code":"ab"}),
    ] {
        assert!(validator.is_valid(&instance), "{instance}");
    }
    for instance in [
        json!({"code":"a"}),
        json!({"pair":["ab",2,3]}),
        json!({"pair":["ab","wrong"]}),
        json!({"card":1}),
        json!({"flag":true}),
    ] {
        assert!(!validator.is_valid(&instance), "{instance}");
    }
    Ok(())
}

#[test]
fn draft7_reference_siblings_and_their_identifiers_stay_inactive()
-> Result<(), Box<dyn std::error::Error>> {
    let result = bundle(
        json!({
            "payload":{"$ref":"#/components/schemas/Code","$id":"bad#identifier","type":"integer","properties":{"ignored":{"$ref":"never-read.json"}}},
            "components":{"schemas":{"Code":{"type":"string"}}}
        }),
        vec![root("/components/schemas/Code", SchemaDialect::Draft7)],
    )?;
    assert!(!result.document.to_string().contains("never-read"));
    let report = analyze(&result.document)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let validator = validator(&result.document)?;
    assert!(validator.is_valid(&json!("text")));
    assert!(!validator.is_valid(&json!(12)));
    Ok(())
}

#[test]
fn mixed_dialect_registry_preserves_modern_reference_siblings()
-> Result<(), Box<dyn std::error::Error>> {
    let result = bundle(
        json!({
            "payload":{"$ref":"#/components/schemas/Modern"},
            "components":{"schemas":{
                "Modern":{"$ref":"#/components/schemas/Base","minLength":2},
                "Base":{"type":"string"},
                "Unused":{"$ref":"never-read.json"}
            }}
        }),
        vec![
            root("/components/schemas/Modern", SchemaDialect::Draft202012),
            root("/components/schemas/Base", SchemaDialect::Draft202012),
            root("/components/schemas/Unused", SchemaDialect::Draft7),
        ],
    )?;
    assert!(!result.document.to_string().contains("never-read"));
    let report = analyze(&result.document)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let validator = validator(&result.document)?;
    assert!(validator.is_valid(&json!("ab")));
    assert!(!validator.is_valid(&json!("a")));
    Ok(())
}

#[test]
fn draft7_metaschema_rejects_bad_dependency_and_tuple_shapes()
-> Result<(), Box<dyn std::error::Error>> {
    for invalid in [
        json!({"dependencies":{"field":3}}),
        json!({"items":[7]}),
        json!({"required":"field"}),
    ] {
        let mut schema = invalid;
        schema["$schema"] = json!(SchemaDialect::Draft7.uri());
        let document = json!({"openapi":"3.1.2","info":{"title":"Invalid","version":"1"},"paths":{},"components":{"schemas":{"Invalid":schema}}});
        let report = analyze(&document)?;
        assert!(!report.valid, "{document}: {:?}", report.diagnostics);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "JSON_SCHEMA")
        );
    }
    Ok(())
}

#[test]
fn conflicting_defaults_for_one_physical_root_are_rejected() {
    let error = bundle(
        json!({"payload":{"type":"string"}}),
        vec![root("/payload", SchemaDialect::Draft202012)],
    );
    assert!(error.is_err_and(|error| {
        error
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "SCHEMA_DIALECT_CONFLICT")
    }));
}

#[test]
fn unknown_keywords_and_non_tuple_additional_items_do_not_resolve_references()
-> Result<(), Box<dyn std::error::Error>> {
    let result = bundle(
        json!({"payload":{
            "type":"array","items":{"type":"string"},
            "additionalItems":{"$id":"invalid#id","$ref":"never-read.json"},
            "$defs":{"literal":{"$ref":"also-never-read.json"}},
            "dependentSchemas":{"literal":{"$ref":"third-never-read.json"}},
            "prefixItems":[{"$ref":"fourth-never-read.json"}]
        }}),
        Vec::new(),
    )?;
    let report = analyze(&result.document)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(
        report
            .references
            .iter()
            .all(|reference| !reference.reference.contains("never-read"))
    );
    Ok(())
}

#[test]
fn dereference_keeps_draft7_validation_after_leaving_its_parent_resource()
-> Result<(), Box<dyn std::error::Error>> {
    let result = bundle(
        json!({"payload":{"type":"object","dependencies":{"id":["name"]}}}),
        Vec::new(),
    )?;
    let graph = octri_parser::execute_document(
        Request {
            operation: Operation::Dereference,
            input: Input {
                uri: "memory://relocated/openapi.json".into(),
                content: Some(result.document.to_string()),
                generated: true,
            },
            options: Options::default(),
        },
        &mut NoIo,
    )?
    .graph
    .ok_or("graph")?
    .to_value(&Default::default())?;
    let schema = &graph["components"]["schemas"]["Payload"];
    let validator = jsonschema::validator_for(schema)?;
    assert!(validator.is_valid(&json!({"id":1,"name":"ok"})));
    assert!(!validator.is_valid(&json!({"id":1})));
    Ok(())
}

#[test]
fn external_reference_only_draft7_documents_keep_resource_boundaries()
-> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options::default();
    options.files.insert(
        SOURCE.into(),
        json!({"payload":{"$ref":"ref.json"}}).to_string(),
    );
    options.files.insert(
        "memory://events/ref.json".into(),
        json!({"$ref":"target.json","type":"integer"}).to_string(),
    );
    options.files.insert(
        "memory://events/target.json".into(),
        json!({"type":"string"}).to_string(),
    );
    let result = bundle_schema_registry_with_dialects(Input {
        uri: "memory://generated/api.json".into(),
        content: Some(json!({"openapi":"3.1.2","info":{"title":"Events","version":"1"},"paths":{},"components":{"schemas":{"Payload":{"$ref":format!("{SOURCE}#/payload")}}}}).to_string()),
        generated: true,
    }, vec![root("/payload", SchemaDialect::Draft7)], Vec::new(), options, &mut NoIo)?;
    let report = analyze(&result.document)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let validator = validator(&result.document)?;
    assert!(validator.is_valid(&json!("ok")));
    assert!(!validator.is_valid(&json!(1)));
    Ok(())
}

#[test]
fn explicit_targets_inside_ignored_siblings_get_their_own_emitted_root()
-> Result<(), Box<dyn std::error::Error>> {
    let result = bundle(
        json!({
            "payload":{"type":"object","properties":{"selected":{"$ref":"#/components/schemas/Reference/definitions/Selected"},"reference":{"$ref":"#/components/schemas/Reference"}}},
            "components":{"schemas":{
                "Reference":{"$ref":"#/components/schemas/Base","definitions":{"Selected":{"type":"integer"},"Unused":{"$ref":"never-read.json"}}},
                "Base":{"type":"string"}
            }}
        }),
        vec![
            root("/components/schemas/Reference", SchemaDialect::Draft7),
            root("/components/schemas/Base", SchemaDialect::Draft7),
        ],
    )?;
    let report = analyze(&result.document)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let validator = validator(&result.document)?;
    assert!(validator.is_valid(&json!({"selected":1,"reference":"ok"})));
    assert!(!validator.is_valid(&json!({"selected":"wrong"})));
    assert!(!result.document.to_string().contains("never-read"));
    Ok(())
}

#[test]
fn a_foreign_openapi_document_controls_its_own_schema_dialect()
-> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options::default();
    options.files.insert(
        SOURCE.into(),
        json!({"payload":{"$ref":"models.json#/components/schemas/Name"}}).to_string(),
    );
    options.files.insert("memory://events/models.json".into(), json!({
        "openapi":"3.1.2", "info":{"title":"Models","version":"1"}, "paths":{},
        "components":{"schemas":{"Name":{"$ref":"#/components/schemas/Base","minLength":2},"Base":{"type":"string"}}}
    }).to_string());
    let result = bundle_schema_registry_with_dialects(Input {
        uri: "memory://generated/api.json".into(),
        content: Some(json!({"openapi":"3.1.2","info":{"title":"Events","version":"1"},"paths":{},"components":{"schemas":{"Payload":{"$ref":format!("{SOURCE}#/payload")}}}}).to_string()),
        generated: true,
    }, vec![root("/payload", SchemaDialect::Draft7)], Vec::new(), options, &mut NoIo)?;
    let report = analyze(&result.document)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let validator = validator(&result.document)?;
    assert!(validator.is_valid(&json!("ab")));
    assert!(!validator.is_valid(&json!("a")));
    Ok(())
}
