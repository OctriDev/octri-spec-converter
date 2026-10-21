use octri_parser::{
    Address, Input, NoIo, Options, SchemaDialect, SchemaRoot, bundle_schema_registry_with_dialects,
};
use serde_json::{Value, json};

#[test]
fn legacy_api_dereference_and_bundle_reload_keep_schema_shapes()
-> Result<(), Box<dyn std::error::Error>> {
    for version in ["2.0", "3.0.4"] {
        let (selector, schemas) = if version == "2.0" {
            ("swagger", "/definitions")
        } else {
            ("openapi", "/components/schemas")
        };
        let mut document = json!({"info":{"title":"Legacy","version":"1"},"paths":{}});
        document[selector] = json!(version);
        let entries = json!({
            "Value":{"type":"string"},
            "Alias":{"$ref":format!("#{schemas}/Value")}
        });
        if version == "2.0" {
            document["definitions"] = entries;
        } else {
            document["components"] = json!({"schemas":entries});
        }
        let report = octri_parser::execute(
            octri_parser::Request {
                operation: octri_parser::Operation::Dereference,
                input: Input {
                    uri: "memory://legacy/api.json".into(),
                    content: Some(document.to_string()),
                    generated: false,
                },
                options: Options::default(),
            },
            &mut NoIo,
        )?;
        let output = report.document.ok_or("missing dereferenced document")?;
        for name in ["Value", "Alias"] {
            assert_eq!(
                output.pointer(&format!("{schemas}/{name}")),
                Some(&json!({"type":"string"})),
                "{version} {name}"
            );
        }
        *document
            .pointer_mut(&format!("{schemas}/Value"))
            .ok_or("missing source schema")? = json!({"$ref":"external.json"});
        let expected = json!({"type":"object","properties":{"name":{"type":"string"}}});
        let bundled = octri_parser::execute(
            octri_parser::Request {
                operation: octri_parser::Operation::Bundle,
                input: Input {
                    uri: "memory://legacy/api.json".into(),
                    content: Some(document.to_string()),
                    generated: false,
                },
                options: Options {
                    files: std::collections::BTreeMap::from([(
                        "external.json".into(),
                        expected.to_string(),
                    )]),
                    ..Default::default()
                },
            },
            &mut NoIo,
        )?
        .document
        .ok_or("missing bundle")?;
        let reloaded = octri_parser::execute(
            octri_parser::Request {
                operation: octri_parser::Operation::Dereference,
                input: Input {
                    uri: "memory://relocated/api.json".into(),
                    content: Some(bundled.to_string()),
                    generated: false,
                },
                options: Options::default(),
            },
            &mut NoIo,
        )?
        .document
        .ok_or("missing relocated output")?;
        assert_eq!(
            reloaded.pointer(&format!("{schemas}/Alias")),
            Some(&expected)
        );
    }
    Ok(())
}

fn bundle(
    version: &str,
    schema: Value,
    mut options: Options,
) -> Result<octri_parser::BundledSource, Box<dyn std::error::Error>> {
    let uri = "memory://source/models.json";
    options.files.insert(uri.into(), json!({"openapi":version,"info":{"title":"Models","version":"1"},"paths":{},"components":{"schemas":{"Selected":schema}}}).to_string());
    let address = Address {
        uri: uri.into(),
        pointer: "/components/schemas/Selected".into(),
    };
    Ok(bundle_schema_registry_with_dialects(
        Input { uri:"memory://output/api.json".into(), generated:true, content:Some(json!({"openapi":"3.1.2","info":{"title":"Envelope","version":"1"},"paths":{},"components":{"schemas":{"Payload":{"$ref":format!("{uri}#/components/schemas/Selected")}}}}).to_string()) },
        vec![SchemaRoot {address, dialect:if version.starts_with("3.0.") { SchemaDialect::Draft202012 } else { SchemaDialect::Draft4 }}], vec![], options, &mut NoIo,
    )?)
}

#[test]
fn legacy_owner_keeps_reference_resource_siblings_inert() -> Result<(), Box<dyn std::error::Error>>
{
    let mut options = Options::default();
    options.files.insert(
        "memory://source/alias.json".into(),
        json!({"$id":"memory://trap/","$ref":"number.json"}).to_string(),
    );
    options.files.insert(
        "memory://source/number.json".into(),
        json!({"type":"integer","minimum":3}).to_string(),
    );
    options.files.insert(
        "memory://trap/number.json".into(),
        json!({"type":"string"}).to_string(),
    );
    let bundle = bundle(
        "3.0.4",
        json!({"$ref":"alias.json","$id":"memory://trap/","$schema":"https://json-schema.org/draft/2020-12/schema","$dynamicRef":"never-read.json"}),
        options,
    )?;
    assert!(
        bundle
            .references
            .iter()
            .all(|edge| edge.kind == "$ref" && edge.resolved)
    );
    assert!(bundle.references.iter().any(|edge| {
        edge.from.address.uri == "memory://source/alias.json"
            && edge
                .target
                .as_ref()
                .is_some_and(|target| target.address.uri == "memory://source/number.json")
    }));
    assert!(!bundle.document.to_string().contains("never-read"));
    assert!(
        !bundle
            .sources
            .iter()
            .any(|source| source.uri.starts_with("memory://trap/"))
    );
    assert!(
        bundle
            .sources
            .iter()
            .any(|source| source.uri == "memory://source/alias.json"
                && source.text.contains("memory://trap/"))
    );
    Ok(())
}

#[test]
fn explicit_nested_json_schema_starts_its_own_resource_scope()
-> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options::default();
    options.files.insert("memory://source/external.json".into(), json!({"Value":{"$schema":"https://json-schema.org/draft/2020-12/schema","$id":"https://models.example.test/","$ref":"number.json"}}).to_string());
    options.files.insert(
        "https://models.example.test/number.json".into(),
        json!({"type":"number","exclusiveMinimum":2}).to_string(),
    );
    let bundle = bundle("3.0.4", json!({"$ref":"external.json#/Value"}), options)?;
    assert!(bundle.references.iter().all(|edge| edge.resolved));
    assert!(bundle.references.iter().any(|edge| {
        edge.target
            .as_ref()
            .is_some_and(|target| target.address.uri == "https://models.example.test/number.json")
    }));
    Ok(())
}

#[test]
fn modern_owner_activates_resource_identity_despite_legacy_import_hint()
-> Result<(), Box<dyn std::error::Error>> {
    for version in ["3.1.2", "3.2.1"] {
        let mut options = Options::default();
        options.files.insert(
            "https://models.example.test/number.json".into(),
            json!({"type":"integer"}).to_string(),
        );
        let bundle = bundle(
            version,
            json!({"$id":"https://models.example.test/","$ref":"number.json","minimum":3}),
            options,
        )?;
        assert!(bundle.references.iter().all(|edge| edge.resolved));
        assert!(bundle.references.iter().any(|edge| {
            edge.target.as_ref().is_some_and(|target| {
                target.address.uri == "https://models.example.test/number.json"
            })
        }));
        assert!(bundle.document.to_string().contains("minimum"));
    }
    Ok(())
}
