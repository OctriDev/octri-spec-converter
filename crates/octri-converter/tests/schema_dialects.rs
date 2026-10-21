use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::json;

#[test]
fn global_draft7_keeps_ignored_reference_siblings_inert_without_bundling()
-> Result<(), Box<dyn std::error::Error>> {
    for version in ["3.1.2", "3.2.1"] {
        let source = json!({"openapi":version,"jsonSchemaDialect":"http://json-schema.org/draft-07/schema#",
        "info":{"title":"Inherited dialect","version":"1"},"paths":{},
        "components":{"schemas":{"Number":{"type":"number"},"Alias":{
            "$ref":"#/components/schemas/Number",
            "properties":{"ignored":{"$schema":"https://dialects.example/custom","xml":{"nodeType":"attribute"}}}
        }}}});
        let report = convert(&ConversionRequest {
            input: Input {
                uri: "memory://fixtures/inherited.json".into(),
                content: Some(source.to_string()),
                generated: false,
            },
            files: Default::default(),
            config: Config {
                bundle: false,
                ..Default::default()
            },
        })?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
        assert_eq!(report.document.as_ref(), Some(&source));
    }
    Ok(())
}

#[test]
fn local_dialect_overrides_global_default_for_active_schema_checks()
-> Result<(), Box<dyn std::error::Error>> {
    let source = json!({"openapi":"3.1.2","jsonSchemaDialect":"http://json-schema.org/draft-07/schema#",
    "info":{"title":"Mixed dialects","version":"1"},"paths":{},
    "components":{"schemas":{"Number":{"type":"number"},"Alias":{
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "$ref":"#/components/schemas/Number",
        "properties":{"active":{"$schema":"https://dialects.example/custom"}}
    }}}});
    let report = convert(&ConversionRequest {
        input: Input {
            uri: "memory://fixtures/mixed.json".into(),
            content: Some(source.to_string()),
            generated: false,
        },
        files: Default::default(),
        config: Config {
            bundle: false,
            ..Default::default()
        },
    })?;
    assert_eq!(report.status, Status::Blocked);
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "SCHEMA_DIALECT_UNSUPPORTED")
    );
    Ok(())
}

#[test]
fn legacy_targets_respect_global_draft7_after_authored_schema_replacements()
-> Result<(), Box<dyn std::error::Error>> {
    let alias = json!({"$ref":"#/components/schemas/Number","type":"string",
        "properties":{"ignored":{"$schema":"https://dialects.example/custom"}}});
    let source = json!({"openapi":"3.1.2","jsonSchemaDialect":"http://json-schema.org/draft-07/schema#",
        "info":{"title":"Legacy dialect","version":"1"},"paths":{},
        "components":{"schemas":{"Number":{"type":"number"},"Alias":alias}}});
    for target in ["2.0", "3.0.0", "3.0.1", "3.0.2", "3.0.3", "3.0.4"] {
        for replacement in [false, true] {
            let mut request = ConversionRequest {
                input: Input {
                    uri: "memory://fixtures/legacy.json".into(),
                    content: Some(source.to_string()),
                    generated: false,
                },
                files: Default::default(),
                config: Config {
                    bundle: false,
                    target_version: Some(target.into()),
                    drop_unsupported: std::collections::BTreeSet::from([
                        "/jsonSchemaDialect".into()
                    ]),
                    ..Default::default()
                },
            };
            if replacement {
                // Overrides apply after migration's forced source bundling.
                // The emitter must still use the API's Draft 7 default.
                request
                    .config
                    .overrides
                    .insert("/components/schemas/Alias".into(), alias.clone());
            }
            let report = convert(&request)?;
            assert!(report.valid, "{target}: {:?}", report.diagnostics);
            assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
            let document = report.document.ok_or("missing output")?;
            let registry_path = if target == "2.0" {
                "definitions"
            } else {
                "components/schemas"
            };
            assert_eq!(
                document.pointer(&format!("/{registry_path}/Alias")),
                Some(&json!({"$ref":format!("#/{registry_path}/Number")}))
            );
            let uri = "https://converted.example/spec.json";
            let registry = jsonschema::Registry::new()
                .add(uri, jsonschema::Resource::from_contents(document))?
                .prepare()?;
            let validator = jsonschema::options()
                .with_registry(&registry)
                .build(&json!({"$ref":format!("{uri}#/{registry_path}/Alias")}))?;
            assert!(validator.is_valid(&json!(42)));
            assert!(!validator.is_valid(&json!("ignored sibling must not activate")));
            request.config.accepted_decisions = report
                .decisions
                .into_iter()
                .map(|decision| decision.id)
                .collect();
            assert_eq!(convert(&request)?.status, Status::Complete);
        }
    }
    Ok(())
}

#[test]
fn unimplemented_schema_dialects_cannot_be_approved_as_converted_contracts()
-> Result<(), Box<dyn std::error::Error>> {
    for uri in [
        "http://json-schema.org/draft-03/schema#",
        "https://dialects.example/custom",
    ] {
        for global in [false, true] {
            let mut schema = json!({"type":"string"});
            let mut source =
                json!({"openapi":"3.1.2","info":{"title":"Custom","version":"1"},"paths":{}});
            if global {
                source["jsonSchemaDialect"] = json!(uri);
            } else {
                schema["$schema"] = json!(uri);
            }
            source["components"] = json!({"schemas":{"Value":schema}});
            let mut request = ConversionRequest {
                input: Input {
                    uri: "memory://fixtures/custom.json".into(),
                    content: Some(source.to_string()),
                    generated: false,
                },
                files: Default::default(),
                config: Config::default(),
            };
            let report = convert(&request)?;
            assert_eq!(
                report.status,
                Status::Blocked,
                "{uri}: {:?}",
                report.decisions
            );
            assert!(
                report
                    .decisions
                    .iter()
                    .any(|decision| decision.code == "SCHEMA_DIALECT_UNSUPPORTED")
            );
            request.config.accepted_decisions = report
                .decisions
                .into_iter()
                .map(|decision| decision.id)
                .collect();
            assert!(!convert(&request)?.handoff_allowed);
        }
    }
    Ok(())
}

#[test]
fn instance_data_cannot_declare_an_active_schema_dialect() -> Result<(), Box<dyn std::error::Error>>
{
    let source = json!({"openapi":"3.1.2","info":{"title":"Literal","version":"1"},"paths":{},
        "components":{"schemas":{"Value":{"type":"object","examples":[{"$schema":"https://dialects.example/custom","$ref":"never-read.json"}]}}}});
    let report = convert(&ConversionRequest {
        input: Input {
            uri: "memory://fixtures/literal.json".into(),
            content: Some(source.to_string()),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    })?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    Ok(())
}

#[test]
fn imported_openapi_custom_defaults_are_preserved_and_block_handoff()
-> Result<(), Box<dyn std::error::Error>> {
    let source = json!({"asyncapi":"3.0.0","info":{"title":"Events","version":"1"},
        "channels":{"events":{"messages":{"event":{"payload":{"$ref":"models.json#/components/schemas/Value"}}}}},
        "operations":{"receive":{"action":"receive","channel":{"$ref":"#/channels/events"}}}});
    let models = json!({"openapi":"3.1.2","jsonSchemaDialect":"https://dialects.example/custom",
        "info":{"title":"Models","version":"1"},"paths":{},"components":{"schemas":{"Value":{"type":"string"}}}});
    let mut request = ConversionRequest {
        input: Input {
            uri: "memory://fixtures/events.json".into(),
            content: Some(source.to_string()),
            generated: false,
        },
        files: std::collections::BTreeMap::from([("models.json".into(), models.to_string())]),
        config: Config::default(),
    };
    request.config.http_mappings.insert("receive".into(), serde_json::from_value(json!({"method":"POST","path":"/events","server":"https://api.example.com","responseStatus":"202","security":[],"payloadLocation":"request"}))?);
    let report = convert(&request)?;
    assert_eq!(report.status, Status::Blocked);
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "SCHEMA_DIALECT_UNSUPPORTED")
    );
    assert!(
        report
            .document
            .ok_or("output")?
            .to_string()
            .contains("https://dialects.example/custom")
    );
    Ok(())
}
