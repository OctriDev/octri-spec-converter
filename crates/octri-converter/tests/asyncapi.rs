use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::json;

#[test]
fn authored_payload_replacements_require_review_and_stay_operation_scoped()
-> Result<(), Box<dyn std::error::Error>> {
    for version in ["2.6.0", "3.0.0"] {
        let payload = json!({
            "schemaFormat": "application/vnd.apache.avro+json;version=1.9.0",
            "schema": {"type": "record", "name": "Event", "fields": []}
        });
        let message = json!({"payload":payload});
        let source = if version.starts_with('2') {
            json!({
                "asyncapi": version,
                "info": {"title": "Events", "version": "1"},
                "channels": {"events": {
                    "publish": {"message": message}, "subscribe": {"message": message}
                }}
            })
        } else {
            json!({
                "asyncapi": version,
                "info": {"title": "Events", "version": "1"},
                "components": {"schemas": {"Unused": {"dependencies": {"id": ["name"]}}}},
                "channels": {"events": {"messages": {"event": message}}},
                "operations": {
                    "send": {"action": "send", "channel": {"$ref": "#/channels/events"}},
                    "receive": {"action": "receive", "channel": {"$ref": "#/channels/events"}}
                }
            })
        };
        let keys = if version.starts_with('2') {
            ["publish events", "subscribe events"]
        } else {
            ["send", "receive"]
        };
        let mut request = ConversionRequest {
            input: Input {
                uri: "memory://fixtures/events.json".into(),
                content: Some(source.to_string()),
                generated: false,
            },
            files: Default::default(),
            config: Config::default(),
        };
        for (index, key) in keys.iter().enumerate() {
            request.config.http_mappings.insert((*key).into(), serde_json::from_value(json!({
                "method": "POST", "path": format!("/events/{index}"),
                "server": "https://api.example.com", "responseStatus": "202",
                "security": [], "payloadLocation": "request", "requestSchema": {"type": "string"}
            }))?);
        }
        // An authored schema in the other direction must not hide this payload.
        let second = request
            .config
            .http_mappings
            .get_mut(keys[1])
            .ok_or("mapping")?;
        second.response_schema = second.request_schema.take();
        let mixed = convert(&request)?;
        assert_eq!(mixed.status, Status::Blocked);
        assert!(
            mixed
                .decisions
                .iter()
                .any(|item| item.code == "ASYNCAPI_PAYLOAD_FORMAT")
        );
        assert!(
            mixed
                .mapping_requests
                .iter()
                .all(|item| item.payload_location_required)
        );

        let second = request
            .config
            .http_mappings
            .get_mut(keys[1])
            .ok_or("mapping")?;
        second.request_schema = second.response_schema.take();
        let replaced = convert(&request)?;
        assert!(replaced.valid, "{:?}", replaced.diagnostics);
        assert_eq!(
            replaced.status,
            Status::NeedsReview,
            "{:?}",
            replaced.decisions
        );
        assert!(
            replaced
                .decisions
                .iter()
                .any(|item| item.code == "ASYNCAPI_PAYLOAD_REPLACED" && item.review_required)
        );
        assert!(
            !replaced
                .decisions
                .iter()
                .any(|item| item.code == "ASYNCAPI_SCHEMA_DIALECT")
        );
        for index in 0..2 {
            let pointer = format!(
                "/paths/~1events~1{index}/post/requestBody/content/application~1json/schema/type"
            );
            assert_eq!(
                replaced
                    .document
                    .as_ref()
                    .and_then(|doc| doc.pointer(&pointer)),
                Some(&json!("string"))
            );
        }
        request.config.accepted_decisions = replaced
            .decisions
            .iter()
            .filter(|item| item.review_required)
            .map(|item| item.id.clone())
            .collect();
        let reviewed = convert(&request)?;
        assert_eq!(reviewed.status, Status::Complete);
        assert!(reviewed.handoff_allowed);
    }
    Ok(())
}

#[test]
fn response_replacement_retains_external_payload_evidence() -> Result<(), Box<dyn std::error::Error>>
{
    let source = json!({
        "asyncapi": "3.0.0", "info": {"title": "Events", "version": "1"},
        "channels": {"events": {}},
        "operations": {"receive": {
            "action": "receive", "channel": {"$ref": "#/channels/events"},
            "messages": [{"$ref": "message.json#/event"}]
        }}
    });
    let mut request = ConversionRequest {
        input: Input { uri: "memory://fixtures/events.json".into(), content: Some(source.to_string()), generated: false },
        files: std::collections::BTreeMap::from([("message.json".into(), json!({"event": {
            "payload": {"type": "array", "items": [{"type": "string"}], "additionalItems": false}
        }}).to_string())]),
        config: Config::default(),
    };
    request.config.http_mappings.insert(
        "receive".into(),
        serde_json::from_value(json!({
            "method": "GET", "path": "/events", "server": "https://api.example.com",
            "responseStatus": "200", "security": [], "payloadLocation": "response",
            "responseSchema": {"type": "boolean"}
        }))?,
    );
    let report = convert(&request)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview);
    let replacement = report
        .decisions
        .iter()
        .find(|item| item.code == "ASYNCAPI_PAYLOAD_REPLACED")
        .ok_or("replacement evidence")?;
    assert_eq!(
        replacement.source.address.uri,
        "memory://fixtures/message.json"
    );
    assert_eq!(replacement.source.address.pointer, "/event/payload");
    assert_eq!(
        report.document.as_ref().and_then(|doc| doc
            .pointer("/paths/~1events/get/responses/200/content/application~1json/schema/type")),
        Some(&json!("boolean"))
    );
    request
        .config
        .http_mappings
        .get_mut("receive")
        .ok_or("mapping")?
        .payload_location = Some("response-typo".into());
    assert_eq!(
        convert(&request)
            .err()
            .ok_or("invalid direction accepted")?
            .code,
        "ASYNCAPI_PAYLOAD_BINDING"
    );
    Ok(())
}

#[test]
fn unused_registry_schemas_do_not_block_mixed_source_and_replacement_payloads()
-> Result<(), Box<dyn std::error::Error>> {
    let mut source = json!({
        "asyncapi":"3.0.0", "info":{"title":"Mixed payloads","version":"1"},
        "components":{"schemas":{
            "Unused":{"dependencies":{"id":["name"]},"$ref":"https://never-read.invalid/schema"},
            "UnusedAvro":{"schemaFormat":"application/vnd.apache.avro+json", "schema":["null","string"]}
        }},
        "channels":{
            "first":{"messages":{"event":{"payload":{"type":"string"}}}},
            "second":{"messages":{"event":{"payload":{
                "schemaFormat":"application/vnd.apache.avro+json",
                "schema":{"type":"record","name":"Event","fields":[]}
            }}}}
        },
        "operations":{
            "one":{"action":"receive","channel":{"$ref":"#/channels/first"}},
            "two":{"action":"receive","channel":{"$ref":"#/channels/second"}}
        }
    });
    let mut request = ConversionRequest {
        input: Input {
            uri: "memory://fixtures/events.json".into(),
            content: Some(source.to_string()),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    };
    for key in ["one", "two"] {
        request.config.http_mappings.insert(
            key.into(),
            serde_json::from_value(json!({
                "method":"POST", "path":format!("/{key}"), "server":"https://api.example.com",
                "responseStatus":"202", "security":[], "payloadLocation":"request"
            }))?,
        );
    }
    request
        .config
        .http_mappings
        .get_mut("two")
        .ok_or("mapping")?
        .request_schema = Some(json!({"type":"string"}));
    let report = convert(&request)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::NeedsReview, "{:?}", report.decisions);
    assert!(
        report
            .decisions
            .iter()
            .all(|decision| decision.classification != octri_converter::Classification::Blocked)
    );
    assert!(
        !report
            .document
            .ok_or("output")?
            .to_string()
            .contains("never-read")
    );
    let omissions: Vec<_> = report
        .decisions
        .iter()
        .filter(|decision| decision.code == "ASYNCAPI_UNUSED_SCHEMA")
        .collect();
    assert_eq!(omissions.len(), 2);
    assert!(omissions.iter().all(|decision| decision.review_required
        && decision.classification == octri_converter::Classification::Omitted
        && decision.source.address.uri == request.input.uri));
    request.config.accepted_decisions = report
        .decisions
        .iter()
        .filter(|decision| decision.review_required)
        .map(|decision| decision.id.clone())
        .collect();
    let acknowledged = convert(&request)?;
    assert_eq!(acknowledged.status, Status::Complete);
    assert!(acknowledged.handoff_allowed);

    // Reaching this component now preserves its Draft 7 dependencies. Other
    // omitted components still require review; preservation clears no review.
    source["components"]["schemas"]["Unused"]
        .as_object_mut()
        .ok_or("schema")?
        .remove("$ref");
    source["channels"]["first"]["messages"]["event"]["payload"] =
        json!({"$ref":"#/components/schemas/Unused"});
    request.input.content = Some(source.to_string());
    let active = convert(&request)?;
    assert_eq!(active.status, Status::NeedsReview);
    assert!(active.valid, "{:?}", active.diagnostics);
    assert!(
        !active
            .decisions
            .iter()
            .any(|decision| decision.classification == octri_converter::Classification::Blocked)
    );
    Ok(())
}

#[test]
fn imported_components_use_their_own_document_dialect() -> Result<(), Box<dyn std::error::Error>> {
    for version in ["3.0.4", "3.1.2", "3.2.1"] {
        let source = json!({
            "asyncapi":"3.0.0", "info":{"title":"Events","version":"1"},
            "channels":{"events":{"messages":{"event":{"payload":{"$ref":"models.json#/components/schemas/Name"}}}}},
            "operations":{"receive":{"action":"receive","channel":{"$ref":"#/channels/events"}}}
        });
        let models = json!({
            "openapi":version, "info":{"title":"Models","version":"1"}, "paths":{},
            "components":{"schemas":{
                "Base":{"type":"string"},
                "Name":{"$ref":"#/components/schemas/Base", "minLength":2}
            }}
        });
        let mut request = ConversionRequest {
            input: Input {
                uri: "memory://fixtures/events.json".into(),
                content: Some(source.to_string()),
                generated: false,
            },
            files: std::collections::BTreeMap::from([("models.json".into(), models.to_string())]),
            config: Config::default(),
        };
        request.config.http_mappings.insert(
            "receive".into(),
            serde_json::from_value(json!({
                "method":"POST", "path":"/events", "server":"https://api.example.com",
                "responseStatus":"202", "security":[], "payloadLocation":"request"
            }))?,
        );
        let report = convert(&request)?;
        assert!(report.valid, "{version}: {:?}", report.diagnostics);
        if version.starts_with("3.0.") {
            assert_eq!(report.status, Status::NeedsReview);
            assert!(
                report
                    .decisions
                    .iter()
                    .any(|decision| decision.code == "IGNORED_REFERENCE_SIBLINGS"
                        && decision.source.address.uri == "memory://fixtures/models.json")
            );
        } else {
            assert_eq!(
                report.status,
                Status::Complete,
                "{version}: {:?}",
                report.decisions
            );
        }
        let output = report.document.ok_or("output")?;
        // Extracting one schema loses the document's embedded resource registry.
        // Register each resource before validating references across components.
        let mut registry = jsonschema::Registry::new();
        for schema in output
            .pointer("/components/schemas")
            .and_then(serde_json::Value::as_object)
            .ok_or("schema components")?
            .values()
        {
            if let Some(id) = schema.get("$id").and_then(serde_json::Value::as_str) {
                registry = registry.add(id, jsonschema::Resource::from_contents(schema.clone()))?;
            }
        }
        let registry = registry
            .add(
                "memory://export/api.json",
                jsonschema::Resource::from_contents(output.clone()),
            )?
            .prepare()?;
        let raw_validator = jsonschema::options().with_registry(&registry).build(&json!({"$ref":"memory://export/api.json#/paths/~1events/post/requestBody/content/application~1json/schema"})).map_err(|error| format!("Raw {version}: {error}\n{output}"))?;
        assert!(raw_validator.is_valid(&json!("ab")));
        assert!(
            raw_validator.is_valid(&json!("a")) == version.starts_with("3.0."),
            "Raw exported {version} schema lost its owning document's dialect"
        );
        let expanded = octri_parser::execute_document(
            octri_parser::Request {
                operation: octri_parser::Operation::Dereference,
                input: Input {
                    uri: "memory://export/api.json".into(),
                    content: Some(output.to_string()),
                    generated: true,
                },
                options: Default::default(),
            },
            &mut octri_parser::NoIo,
        )?
        .graph
        .ok_or("graph")?
        .to_value(&Default::default())?;
        let schema = expanded
            .pointer("/paths/~1events/post/requestBody/content/application~1json/schema")
            .ok_or("request schema")?;
        let validator = jsonschema::options()
            .with_registry(&registry)
            .build(schema)?;
        assert!(validator.is_valid(&json!("ab")));
        assert_eq!(validator.is_valid(&json!("a")), version.starts_with("3.0."));
    }
    Ok(())
}

#[test]
fn asyncapi_draft7_constraints_preserve_instance_validation_in_every_modern_target()
-> Result<(), Box<dyn std::error::Error>> {
    for target in ["3.1.0", "3.1.1", "3.1.2", "3.2.0", "3.2.1"] {
        for (payload, accepted, rejected) in [
            (
                json!({"type":"object","dependencies":{"creditCard":["billingAddress"]}}),
                json!({"creditCard":1,"billingAddress":"address"}),
                json!({"creditCard":1}),
            ),
            (
                json!({"type":"object","dependencies":{"flag":{"required":["name"],"properties":{"name":{"type":"string","minLength":2}}}}}),
                json!({"flag":true,"name":"ab"}),
                json!({"flag":true,"name":"a"}),
            ),
            (
                json!({"type":"array","items":[{"type":"string"}],"additionalItems":false}),
                json!(["ok"]),
                json!(["ok", 2]),
            ),
            (
                json!({"$ref":"#/components/schemas/Name","type":"integer"}),
                json!("a"),
                json!(3),
            ),
            (
                json!({"$ref":"#/components/schemas/Name","minLength":2}),
                json!("a"),
                json!(3),
            ),
            (
                json!({"$ref":"#/components/schemas/Name","pattern":"^A"}),
                json!("z"),
                json!(3),
            ),
        ] {
            let mut source = json!({"asyncapi":"3.0.0","info":{"title":"Events","version":"1"},"channels":{"events":{"address":"events","messages":{"event":{"payload":payload}}}},"operations":{"receive":{"action":"receive","channel":{"$ref":"#/channels/events"}}}});
            if payload.get("$ref").is_some() {
                source["components"] = json!({"schemas":{"Name":{"type":"string"}}});
            }
            let mut config = Config {
                target_version: Some(target.into()),
                ..Default::default()
            };
            config.http_mappings.insert("receive".into(),serde_json::from_value(json!({"method":"POST","path":"/events","server":"https://api.example.com","responseStatus":"202","security":[],"payloadLocation":"request"}))?);
            let report = convert(&ConversionRequest {
                input: Input {
                    uri: "memory://fixtures/events.json".into(),
                    content: Some(source.to_string()),
                    generated: false,
                },
                files: Default::default(),
                config,
            })?;
            assert!(report.valid, "{target}: {:?}", report.diagnostics);
            assert_eq!(
                report.status,
                Status::Complete,
                "{target}: {:?}",
                report.decisions
            );
            assert!(report.handoff_allowed);
            let output = report.document.ok_or("output")?;
            assert_eq!(output["openapi"], target);
            let expanded = octri_parser::execute_document(
                octri_parser::Request {
                    operation: octri_parser::Operation::Dereference,
                    input: Input {
                        uri: "memory://relocated/api.json".into(),
                        content: Some(output.to_string()),
                        generated: true,
                    },
                    options: Default::default(),
                },
                &mut octri_parser::NoIo,
            )?
            .graph
            .ok_or("graph")?
            .to_value(&Default::default())?;
            let schema = expanded
                .pointer("/paths/~1events/post/requestBody/content/application~1json/schema")
                .ok_or("payload schema")?;
            let validator = jsonschema::validator_for(schema)?;
            assert!(
                validator.is_valid(&accepted),
                "{target}: {payload}: {accepted}"
            );
            assert!(
                !validator.is_valid(&rejected),
                "{target}: {payload}: {rejected}"
            );
        }
    }
    Ok(())
}

#[test]
fn asyncapi_multiformat_payloads_unwrap_at_physical_source_locations()
-> Result<(), Box<dyn std::error::Error>> {
    for (format, supported) in [
        ("application/schema+json;version=draft-07", true),
        ("application/schema+json;version=draft-2019-09", true),
        ("application/vnd.aai.asyncapi+yaml;version=3.0.0", true),
        ("application/vnd.apache.avro+json;version=1.9.0", false),
        ("application/not-json-schema+json", false),
    ] {
        let source = json!({"asyncapi":"3.0.0","info":{"title":"Events","version":"1"},"channels":{"events":{"address":"events","messages":{"event":{"payload":{"schemaFormat":format,"schema":{"type":"object","required":["id"],"properties":{"id":{"type":"integer","minimum":1}}}}}}}},"operations":{"receive":{"action":"receive","channel":{"$ref":"#/channels/events"}}}});
        let mut config = Config::default();
        config.http_mappings.insert("receive".into(), serde_json::from_value(json!({"method":"POST","path":"/events","server":"https://api.example.com","responseStatus":"202","security":[],"payloadLocation":"request"}))?);
        let report = convert(&ConversionRequest {
            input: Input {
                uri: "memory://fixtures/events.json".into(),
                content: Some(source.to_string()),
                generated: false,
            },
            files: Default::default(),
            config,
        })?;
        if !supported {
            assert_eq!(report.status, Status::Blocked);
            assert!(
                report
                    .decisions
                    .iter()
                    .any(|decision| decision.code == "ASYNCAPI_PAYLOAD_FORMAT")
            );
            continue;
        }
        assert!(report.valid, "{format}: {:?}", report.diagnostics);
        assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
        let document = report.document.ok_or("missing document")?;
        let analyzed = octri_parser::execute_document(
            octri_parser::Request {
                operation: octri_parser::Operation::Dereference,
                input: Input {
                    uri: "memory://export/api.json".into(),
                    content: Some(document.to_string()),
                    generated: true,
                },
                options: Default::default(),
            },
            &mut octri_parser::NoIo,
        )?;
        let document = analyzed
            .graph
            .ok_or("missing graph")?
            .to_value(&Default::default())?;
        let schema = document
            .pointer("/paths/~1events/post/requestBody/content/application~1json/schema")
            .ok_or("missing payload")?;
        assert!(schema.get("schemaFormat").is_none());
        let validator = jsonschema::validator_for(schema)?;
        assert!(validator.is_valid(&json!({"id":1})));
        assert!(!validator.is_valid(&json!({"id":0})));
        assert!(!validator.is_valid(&json!({})));
    }
    Ok(())
}

#[test]
fn asyncapi_channels_cannot_invent_http_endpoints() -> Result<(), Box<dyn std::error::Error>> {
    let document = json!({"asyncapi":"3.0.0","info":{"title":"Events","version":"1"},"channels":{"pets":{"address":"pets/{tenant}","messages":{"pet":{"payload":{"$ref":"models.json#/Pet"}}}}},"operations":{"consume":{"action":"receive","channel":{"$ref":"#/channels/pets"}}}});
    let mut input=ConversionRequest{input:Input{uri:"memory://fixtures/asyncapi.json".into(),content:Some(document.to_string()),generated:false},files:std::collections::BTreeMap::from([("models.json".into(),json!({"Pet":{"type":"object","properties":{"parent":{"$ref":"#/Pet"},"example":{"type":"object","example":{"$ref":"literal"}}}}}).to_string())]),config:Config::default()};
    let blocked = convert(&input)?;
    assert_eq!(blocked.status, Status::Blocked);
    assert!(
        blocked
            .decisions
            .iter()
            .any(|decision| decision.code == "HTTP_MAPPING_REQUIRED")
    );
    input.config.http_mappings.insert("consume".into(),serde_json::from_value(json!({"method":"POST","path":"/pets/events","server":"https://api.example.com","responseStatus":"202","security":[],"payloadLocation":"request"}))?);
    let converted = convert(&input)?;
    assert!(converted.valid, "{:?}", converted.diagnostics);
    assert_eq!(
        converted.status,
        Status::Complete,
        "{:?}",
        converted.decisions
    );
    assert!(
        converted
            .document
            .ok_or("missing output")?
            .pointer("/paths/~1pets~1events/post/requestBody/content/application~1json/schema/$ref")
            .is_some()
    );
    Ok(())
}

#[test]
fn asyncapi_2_requires_an_explicit_payload_direction() -> Result<(), Box<dyn std::error::Error>> {
    let document = json!({"asyncapi":"2.6.0","info":{"title":"Events","version":"1"},"channels":{"pets":{"publish":{"message":{"payload":{"type":"string"}}}}}});
    let mut input = ConversionRequest {
        input: Input {
            uri: "memory://fixtures/asyncapi.json".into(),
            content: Some(document.to_string()),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    };
    input.config.http_mappings.insert("publish pets".into(),serde_json::from_value(json!({"method":"POST","path":"/pets/events","server":"https://api.example.com","responseStatus":"202","security":[]}))?);
    let blocked = convert(&input)?;
    assert_eq!(blocked.status, Status::Blocked);
    assert!(
        blocked
            .decisions
            .iter()
            .any(|decision| decision.code == "ASYNCAPI_PAYLOAD_BINDING")
    );
    Ok(())
}

#[test]
fn asyncapi_schema_resources_preserve_anchors_and_dynamic_references()
-> Result<(), Box<dyn std::error::Error>> {
    let document = json!({"asyncapi":"3.0.0","info":{"title":"Tree events","version":"1"},
        "components":{"schemas":{"Node":{"$schema":"https://json-schema.org/draft/2020-12/schema","$id":"https://schemas.example/node","$dynamicAnchor":"node","type":"object","properties":{"child":{"$dynamicRef":"#node"},"label":{"$ref":"label#label"}},"example":{"$ref":"inert"}},"Label":{"$schema":"https://json-schema.org/draft/2020-12/schema","$id":"https://schemas.example/label","$anchor":"label","type":"string"}}},
        "channels":{"trees":{"address":"trees","messages":{"tree":{"payload":{"$ref":"https://schemas.example/node"}}}}},
        "operations":{"consume":{"action":"receive","channel":{"$ref":"#/channels/trees"}}}});
    let mut input = ConversionRequest {
        input: Input {
            uri: "memory://fixtures/asyncapi.json".into(),
            content: Some(document.to_string()),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    };
    input.config.http_mappings.insert("consume".into(),serde_json::from_value(json!({"method":"POST","path":"/trees","server":"https://api.example.com","responseStatus":"202","security":[],"payloadLocation":"request"}))?);
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("missing output")?;
    assert!(
        document
            .to_string()
            .contains("https://schemas.example/node#node")
    );
    let reparsed = octri_parser::execute(
        octri_parser::Request {
            operation: octri_parser::Operation::Analyze,
            input: Input {
                uri: "memory://new-location/api.json".into(),
                content: Some(document.to_string()),
                generated: true,
            },
            options: Default::default(),
        },
        &mut octri_parser::NoIo,
    )?;
    assert!(reparsed.valid, "{:?}", reparsed.diagnostics);
    Ok(())
}

#[test]
fn draft7_tuple_and_dependency_downgrades_require_explicit_loss_review()
-> Result<(), Box<dyn std::error::Error>> {
    for target in ["2.0", "3.0.0", "3.0.1", "3.0.2", "3.0.3", "3.0.4"] {
        let source = json!({"asyncapi":"3.0.0","info":{"title":"Events","version":"1"},
            "channels":{"events":{"messages":{"event":{"payload":{"type":"object","dependencies":{"card":["billing"]},"properties":{"pair":{"type":"array","items":[{"type":"string"}],"additionalItems":false}}}}}}},
            "operations":{"receive":{"action":"receive","channel":{"$ref":"#/channels/events"}}}});
        let mut request = ConversionRequest {
            input: Input {
                uri: "memory://fixtures/draft7.json".into(),
                content: Some(source.to_string()),
                generated: false,
            },
            files: Default::default(),
            config: Config::default(),
        };
        request.config.target_version = Some(target.into());
        request.config.http_mappings.insert("receive".into(), serde_json::from_value(json!({"method":"POST","path":"/events","server":"https://api.example.com","responseStatus":"202","security":[],"payloadLocation":"request"}))?);
        let report = convert(&request)?;
        assert_eq!(
            report.status,
            Status::Blocked,
            "{target}: {:?}",
            report.decisions
        );
        assert!(!report.handoff_allowed);
        for suffix in ["/dependencies", "/items"] {
            assert!(
                report
                    .decisions
                    .iter()
                    .any(|decision| decision.classification
                        == octri_converter::Classification::Blocked
                        && decision.source.address.pointer.ends_with(suffix)),
                "{target}: {suffix}: {:?}",
                report.decisions
            );
        }
        // Acknowledging warnings cannot approve an unrepresentable contract.
        request.config.accepted_decisions = report
            .decisions
            .into_iter()
            .map(|decision| decision.id)
            .collect();
        assert!(!convert(&request)?.handoff_allowed);
    }
    Ok(())
}

#[test]
fn draft7_unknown_schema_keywords_remain_literal_during_version_emission()
-> Result<(), Box<dyn std::error::Error>> {
    let source = json!({"asyncapi":"3.0.0","info":{"title":"Events","version":"1"},
        "channels":{"events":{"messages":{"event":{"payload":{"type":"object","$defs":{"Literal":{"$ref":"never-read.json","xml":{"nodeType":"attribute"}}}}}}}},
        "operations":{"receive":{"action":"receive","channel":{"$ref":"#/channels/events"}}}});
    let mut request = ConversionRequest {
        input: Input {
            uri: "memory://fixtures/draft7.json".into(),
            content: Some(source.to_string()),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    };
    request.config.target_version = Some("3.1.2".into());
    request.config.http_mappings.insert("receive".into(), serde_json::from_value(json!({"method":"POST","path":"/events","server":"https://api.example.com","responseStatus":"202","security":[],"payloadLocation":"request"}))?);
    let report = convert(&request)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let output = report.document.ok_or("output")?.to_string();
    assert!(output.contains("never-read.json"));
    assert!(output.contains("nodeType"));
    Ok(())
}
