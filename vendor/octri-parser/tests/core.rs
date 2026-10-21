#![allow(clippy::unwrap_used, clippy::panic)]
use octri_parser::{
    CircularPolicy, GraphNode, Input, NoIo, Operation, Options, Report, Request, execute,
};
use serde_json::{Value, json};

fn api(version: &str, schemas: Value) -> Value {
    let mut value = json!({"info":{"title":"Pets","version":"1"},"paths":{}});
    if version == "2.0" {
        value["swagger"] = json!(version);
        value["definitions"] = schemas;
    } else {
        value["openapi"] = json!(version);
        value["components"] = json!({"schemas":schemas});
    }
    value
}
fn request(operation: Operation, document: Value) -> Request {
    Request {
        operation,
        input: Input {
            uri: "memory://octri/openapi.json".into(),
            content: Some(document.to_string()),
            generated: false,
        },
        options: Options {
            validate: octri_parser::ValidationOptions {
                mode: octri_parser::ValidationMode::Strict,
                ..Default::default()
            },
            ..Default::default()
        },
    }
}
fn run(request: Request) -> Report {
    execute(request, &mut NoIo).unwrap_or_else(|e| panic!("{e:?}"))
}
fn schema<'a>(document: &'a Value, name: &str) -> &'a Value {
    &document["components"]["schemas"][name]
}

#[test]
fn supports_each_dialect_and_structural_validation() {
    for version in ["2.0", "3.0.4", "3.1.2", "3.2.0"] {
        let report = run(request(
            Operation::Analyze,
            api(version, json!({"Pet":{"type":"string"}})),
        ));
        assert!(report.valid, "{version}: {:?}", report.diagnostics);
    }
    let mut invalid = api("3.1.2", json!({}));
    invalid["info"] = json!({"version":"1"});
    let report = run(request(Operation::Analyze, invalid));
    assert!(!report.valid);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "OPENAPI_SCHEMA")
    );
}

#[test]
fn preserves_source_and_shared_graph_identity() {
    let original = api(
        "3.1.2",
        json!({"Pet":{"type":"string"},"A":{"$ref":"#/components/schemas/Pet"},"B":{"$ref":"#/components/schemas/Pet"}}),
    );
    let report = run(request(Operation::Dereference, original.clone()));
    let graph = report.graph.as_ref().unwrap();
    let GraphNode::Object(root) = &graph.nodes[graph.root] else {
        panic!()
    };
    let GraphNode::Object(components) = &graph.nodes[root["components"]] else {
        panic!()
    };
    let GraphNode::Object(schemas) = &graph.nodes[components["schemas"]] else {
        panic!()
    };
    assert_eq!(schemas["A"], schemas["B"]);
    assert_eq!(report.sources[0].document, original);
    assert_eq!(
        schema(report.document.as_ref().unwrap(), "A")["type"],
        "string"
    );
}

#[test]
fn circular_policies_have_explicit_graph_semantics() {
    let document = api(
        "3.1.2",
        json!({"Pet":{"type":"object","properties":{"child":{"$ref":"#/components/schemas/Pet"}}}}),
    );
    let report = run(request(Operation::Dereference, document.clone()));
    assert!(report.graph.as_ref().unwrap().circular);
    assert!(report.document.is_none());
    let mut ignored = request(Operation::Dereference, document.clone());
    ignored.options.dereference.circular = CircularPolicy::Ignore;
    let ignored = run(ignored);
    assert!(ignored.document.is_some());
    let mut denied = request(Operation::Dereference, document);
    denied.options.dereference.circular = CircularPolicy::Error;
    assert!(
        execute(denied, &mut NoIo)
            .unwrap_err()
            .diagnostics
            .iter()
            .any(|d| d.code == "CIRCULAR_REFERENCE")
    );
}

#[test]
fn special_pointer_keys_decode_once() {
    for key in ["a/b", "a~b", "a#b", "a%b", "a%23b", "a b", "a?b", "a😀b"] {
        let token = key.replace('~', "~0").replace('/', "~1");
        let encoded: String = token
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"~-._".contains(&b) {
                    char::from(b).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        let report = run(request(
            Operation::Dereference,
            api(
                "3.1.2",
                json!({key:{"type":"string"},"A":{"$ref":format!("#/components/schemas/{encoded}")}}),
            ),
        ));
        assert_eq!(
            schema(report.document.as_ref().unwrap(), "A")["type"],
            "string",
            "{key}"
        );
    }
}

#[test]
fn malformed_fragments_are_structured_for_every_resolving_operation() {
    let document = api(
        "3.1.2",
        json!({"a/b":{"type":"string"},"A":{"$ref":"#/components/schemas/a%"}}),
    );
    for operation in [
        Operation::Resolve,
        Operation::Bundle,
        Operation::Dereference,
        Operation::Validate,
        Operation::Inspect,
        Operation::Analyze,
    ] {
        let result = execute(request(operation, document.clone()), &mut NoIo);
        let diagnostics = match result {
            Ok(report) => {
                assert!(!report.valid);
                report.diagnostics
            }
            Err(error) => error.diagnostics,
        };
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == "INVALID_REFERENCE_URI")
        );
    }
}

#[test]
fn literal_payloads_are_not_reference_programs() {
    let document = api(
        "3.1.2",
        json!({"Pet":{"type":"object","example":{"$ref":"bad%"},"default":{"$ref":"secret.json"},"x-meta":{"$ref":"missing.json"},"enum":[{"$ref":"nope"}]}}),
    );
    let result = run(request(Operation::Bundle, document.clone()));
    assert!(result.references.is_empty());
    assert_eq!(result.document.unwrap(), document);
}

#[test]
fn nested_schema_ids_and_anchors_resolve_without_io() {
    let document = api(
        "3.1.2",
        json!({"A":{"$id":"https://example.test/a.json","type":"object","properties":{"b":{"$ref":"b.json#B"}},"$defs":{"B":{"$id":"b.json","$anchor":"B","type":"string"}}}}),
    );
    let result = run(request(Operation::Dereference, document));
    assert_eq!(
        schema(result.document.as_ref().unwrap(), "A")["properties"]["b"]["type"],
        "string"
    );
}

#[test]
fn schema_siblings_remain_conjunctions() {
    let document = api(
        "3.1.2",
        json!({"Pet":{"type":"string","maxLength":10},"A":{"$ref":"#/components/schemas/Pet","maxLength":2}}),
    );
    let result = run(request(Operation::Dereference, document));
    let value = schema(result.document.as_ref().unwrap(), "A");
    assert_eq!(value["maxLength"], 2);
    assert_eq!(value["allOf"][0]["maxLength"], 10);
}

#[test]
fn bundles_external_schema_resources_and_survives_new_uri() {
    let document = api("3.1.2", json!({"A":{"$ref":"tree.json#/$defs/label"}}));
    let mut input = request(Operation::Bundle, document);
    input.options.files.insert("tree.json".into(), json!({"$defs":{"label":{"type":"string"}},"type":"object","properties":{"label":{"$ref":"#/$defs/label"}}}).to_string());
    let bundled = run(input).document.unwrap();
    let mut moved = request(Operation::Dereference, bundled);
    moved.input.uri = "memory://elsewhere/moved.json".into();
    let moved = run(moved);
    assert_eq!(
        schema(moved.document.as_ref().unwrap(), "A")["type"],
        "string"
    );
    assert_eq!(moved.sources.len(), 1);
}

#[test]
fn bundle_handles_refs_escaping_nested_schema_resource() {
    for target in [json!({"type":"string"}), json!(false)] {
        let document = api(
            "3.1.2",
            json!({"A":{"$id":"https://example.test/A","properties":{"b":{"$ref":"memory://octri/openapi.json#/components/schemas/B"}}},"B":target}),
        );
        let bundled = run(request(Operation::Bundle, document)).document.unwrap();
        let mut moved = request(Operation::Analyze, bundled);
        moved.input.uri = "memory://fresh/new.json".into();
        let result = run(moved);
        assert!(result.valid, "{:?}", result.diagnostics);
    }
}

#[test]
fn rejects_duplicate_keys_tags_and_alias_expansion_limits() {
    for (text, code) in [
        ("openapi: 3.1.2\nopenapi: 3.1.2\n", "DUPLICATE_KEY"),
        ("openapi: !execute 3.1.2\n", "UNSUPPORTED_YAML_TAG"),
        (
            "{\"openapi\":\"3.1.2\",\"openapi\":\"3.1.2\"}",
            "DUPLICATE_KEY",
        ),
    ] {
        let mut input = request(Operation::Analyze, json!({}));
        input.input.uri = "memory://octri/openapi.yaml".into();
        input.input.content = Some(text.into());
        assert!(
            run(input).diagnostics.iter().any(|d| d.code == code),
            "{code}"
        );
    }
    let mut input = request(Operation::Analyze, json!({}));
    input.input.uri = "memory://octri/openapi.yaml".into();
    input.input.content = Some(
        "openapi: 3.1.2\ninfo: &info {title: Pets, version: '1'}\npaths: {}\nx-alias: *info\n"
            .into(),
    );
    input.options.limits.max_aliases = 0;
    assert!(
        run(input)
            .diagnostics
            .iter()
            .any(|d| d.code == "LIMIT_ALIASES")
    );
}

#[test]
fn distinguishes_boolean_schema_dialects_and_keyword_types() {
    assert!(run(request(Operation::Analyze, api("3.1.2", json!({"A":true})))).valid);
    assert!(!run(request(Operation::Analyze, api("3.0.4", json!({"A":true})))).valid);
    let result = run(request(
        Operation::Analyze,
        api("3.1.2", json!({"A":{"type":"string","minLength":"bad"}})),
    ));
    assert!(result.diagnostics.iter().any(|d| d.code == "JSON_SCHEMA"));
}

#[test]
fn inspection_merges_parameters_security_and_oas32_methods() {
    let document = json!({"openapi":"3.2.0","info":{"title":"Pets","version":"1"},"security":[{"Key":[]}],"components":{"securitySchemes":{"Key":{"type":"apiKey","name":"key","in":"header"}}},"paths":{"/pets/{id}":{"parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"string"}}],"query":{"operationId":"find","security":[],"parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"integer"}}],"responses":{"200":{"description":"OK"}}},"additionalOperations":{"PURGE":{"responses":{"204":{"description":"OK"}}}}}}});
    let report = run(request(Operation::Inspect, document));
    let inspection = report.inspection.unwrap();
    assert_eq!(inspection.statistics.operations, 2);
    let query = inspection
        .operations
        .iter()
        .find(|o| o.method == "QUERY")
        .unwrap();
    assert!(query.security.is_empty());
    assert_eq!(query.parameters[0]["schema"]["type"], "integer");
    assert!(inspection.operations.iter().any(|o| o.method == "PURGE"));
}

#[test]
fn semantic_validation_collects_duplicate_ids_and_path_errors() {
    let document = json!({"openapi":"3.1.2","info":{"title":"Pets","version":"1"},"paths":{"/pets/{id}":{"get":{"operationId":"same","responses":{"200":{"description":"OK"}}},"post":{"operationId":"same","responses":{"200":{"description":"OK"}}}}}});
    let report = run(request(Operation::Analyze, document));
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "DUPLICATE_OPERATION_ID" && !d.related.is_empty())
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "MISSING_PATH_PARAMETER")
    );
}

#[test]
fn reference_only_cycles_preserve_edges_when_ignored() {
    let document = api(
        "3.1.2",
        json!({"A":{"$ref":"#/components/schemas/B"},"B":{"$ref":"#/components/schemas/A"}}),
    );
    let mut ignored = request(Operation::Dereference, document.clone());
    ignored.options.dereference.circular = CircularPolicy::Ignore;
    assert_eq!(run(ignored).document.unwrap(), document);
    let preserved = run(request(Operation::Dereference, document.clone()))
        .document
        .unwrap();
    assert_eq!(schema(&preserved, "A")["$ref"], "#/components/schemas/A");
    assert_eq!(schema(&preserved, "B")["$ref"], "#/components/schemas/A");
    let mut denied = request(Operation::Dereference, document);
    denied.options.dereference.circular = CircularPolicy::Error;
    assert!(execute(denied, &mut NoIo).is_err());
}

#[test]
fn standalone_external_ref_schema_keeps_sibling_constraints() {
    let mut input = request(
        Operation::Dereference,
        api("3.1.2", json!({"A":{"$ref":"name.json"}})),
    );
    input.options.files.insert(
        "name.json".into(),
        json!({"$ref":"base.json","minLength":2}).to_string(),
    );
    input
        .options
        .files
        .insert("base.json".into(), json!({"type":"string"}).to_string());
    let result = run(input);
    let value = schema(result.document.as_ref().unwrap(), "A");
    assert_eq!(value["minLength"], 2);
    assert_eq!(value["allOf"][0]["type"], "string");
}

#[test]
fn external_container_fragment_discovers_nested_schema_refs() {
    let mut input = request(
        Operation::Dereference,
        api("3.1.2", json!({"A":{"$ref":"models.json#/Pet"}})),
    );
    input.options.files.insert(
        "models.json".into(),
        json!({"Pet":{"type":"object","properties":{"name":{"$ref":"name.json"}}}}).to_string(),
    );
    input
        .options
        .files
        .insert("name.json".into(), json!({"type":"string"}).to_string());
    let result = run(input);
    assert_eq!(
        schema(result.document.as_ref().unwrap(), "A")["properties"]["name"]["type"],
        "string"
    );
}

#[test]
fn yaml_roundtrip_and_output_limits_are_explicit() {
    let document = api(
        "3.1.2",
        json!({"Pet":{"type":"string","enum":["null","true","42",null]}}),
    );
    let text = octri_parser::serialize_yaml(&document, &Default::default()).unwrap();
    let mut input = request(Operation::Parse, json!({}));
    input.input.uri = "memory://octri/pets.yaml".into();
    input.input.content = Some(text);
    assert_eq!(run(input).document.unwrap(), document);
    let limits = octri_parser::Limits {
        max_output_bytes: 10,
        ..Default::default()
    };
    assert!(octri_parser::serialize(&document, &limits).is_err());
    assert!(octri_parser::serialize_yaml(&document, &limits).is_err());
}

#[test]
fn retrieval_budget_stops_missing_file_fanout_before_io() {
    #[derive(Debug, Default)]
    struct Counter {
        calls: usize,
    }
    impl octri_parser::Resolver for Counter {
        fn read(
            &mut self,
            uri: &str,
            _: octri_parser::ReadContext<'_>,
        ) -> octri_parser::Result<String> {
            self.calls += 1;
            Err(octri_parser::ParserError::new(
                "NOT_FOUND",
                "No file",
                octri_parser::Address {
                    uri: uri.into(),
                    pointer: String::new(),
                },
            ))
        }
    }
    let mut input = request(
        Operation::Analyze,
        api(
            "3.1.2",
            json!({"A":{"$ref":"a.json"},"B":{"$ref":"b.json"},"C":{"$ref":"c.json"}}),
        ),
    );
    input.options.limits.max_documents = 2;
    let mut resolver = Counter::default();
    let report = execute(input, &mut resolver).unwrap();
    assert_eq!(resolver.calls, 1);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "LIMIT_DOCUMENTS")
    );
}

#[test]
fn callback_expressions_are_not_path_template_parameters() {
    let document = json!({"openapi":"3.1.2","info":{"title":"Pets","version":"1"},"paths":{"/register":{"post":{"responses":{"200":{"description":"OK"}},"callbacks":{"done":{"{$request.body#/callbackUrl}":{"post":{"responses":{"200":{"description":"OK"}}}}}}}}}});
    let report = run(request(Operation::Analyze, document));
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(
        report
            .inspection
            .unwrap()
            .operations
            .iter()
            .any(|op| op.kind == "callback")
    );
}

#[test]
fn custom_dialect_vocabulary_is_preserved_without_false_standard_assertions() {
    let document = api(
        "3.1.2",
        json!({"Pet":{"$schema":"https://example.test/custom","type":"custom-type","minLength":"custom-meaning"}}),
    );
    let report = run(request(Operation::Analyze, document.clone()));
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "CUSTOM_DIALECT")
    );
    assert_eq!(report.document.unwrap(), document);
}

#[test]
fn swagger_semantics_include_required_properties_arrays_and_file_media() {
    let document = json!({"swagger":"2.0","info":{"title":"Pets","version":"1"},"paths":{"/pets":{"post":{"parameters":[{"name":"upload","in":"formData","type":"file"}],"responses":{"600":{"description":"Invalid range"}}}}},"definitions":{"Pet":{"type":"object","required":["missing"],"properties":{}},"Array":{"type":"array"}}});
    let report = run(request(Operation::Analyze, document));
    for code in [
        "FILE_CONSUMES_REQUIRED",
        "INVALID_RESPONSE_STATUS",
        "UNDECLARED_REQUIRED_PROPERTY",
        "ARRAY_ITEMS_REQUIRED",
    ] {
        assert!(
            report.diagnostics.iter().any(|d| d.code == code),
            "{code}: {:?}",
            report.diagnostics
        );
    }
}

#[test]
fn reference_targets_must_match_their_openapi_position() {
    for target in ["#/info", "#/info/title"] {
        let document = json!({"openapi":"3.1.2","info":{"title":"Pets","version":"1"},"paths":{"/pets":{"get":{"responses":{"200":{"$ref":target}}}}}});
        let report = run(request(Operation::Analyze, document));
        assert!(!report.valid);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "REFERENCE_TARGET_SCHEMA" && !d.related.is_empty())
        );
    }
}

#[test]
fn shared_subtree_export_checks_bytes_before_cloning() {
    use octri_parser::{Address, Graph};
    use std::collections::BTreeMap;
    let graph = Graph {
        root: 0,
        nodes: vec![
            GraphNode::Object(BTreeMap::from([
                ("a".into(), 1),
                ("b".into(), 1),
                ("c".into(), 1),
            ])),
            GraphNode::Scalar(json!("x".repeat(1000))),
        ],
        origins: vec![Address::default(); 2],
        circular: false,
    };
    let limits = octri_parser::Limits {
        max_output_bytes: 1500,
        ..Default::default()
    };
    let error = graph.to_value(&limits).unwrap_err();
    assert!(
        error
            .diagnostics
            .iter()
            .any(|d| d.code == "LIMIT_OUTPUT_BYTES")
    );
}

#[test]
fn yaml_alias_bytes_are_bounded_before_expansion() {
    let text = format!(
        "openapi: 3.1.2\ninfo: {{title: Pets, version: '1'}}\npaths: {{}}\nx-base: &base '{}'\nx-a: *base\nx-b: *base\n",
        "x".repeat(1000)
    );
    let mut input = request(Operation::Analyze, json!({}));
    input.input.uri = "memory://octri/openapi.yaml".into();
    input.input.content = Some(text);
    input.options.limits.max_total_bytes = 2000;
    assert!(
        run(input)
            .diagnostics
            .iter()
            .any(|d| d.code == "LIMIT_TOTAL_BYTES")
    );
}

#[test]
fn diagnostic_exhaustion_never_reports_incomplete_validation_as_valid() {
    let document = api(
        "3.1.2",
        json!({"A":{"$ref":"a.json"},"B":{"$ref":"b.json"}}),
    );
    let mut input = request(Operation::Analyze, document);
    input.options.resolve.external = false;
    input.options.limits.max_diagnostics = 1;
    let report = run(input);
    assert!(!report.valid);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].code, "LIMIT_DIAGNOSTICS");
}

#[test]
fn referenced_oauth_security_scheme_is_resolved_before_scope_checks() {
    let document = json!({"openapi":"3.1.2","info":{"title":"Pets","version":"1"},"paths":{"/pets":{"get":{"security":[{"OAuth":["read"]}],"responses":{"200":{"description":"OK"}}}}},"components":{"securitySchemes":{"OAuth":{"$ref":"oauth.json"}}}});
    let mut input = request(Operation::Analyze, document);
    input.options.files.insert("oauth.json".into(),json!({"type":"oauth2","flows":{"clientCredentials":{"tokenUrl":"https://auth.example.test/token","scopes":{"read":"Read access"}}}}).to_string());
    let report = run(input);
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(
        report.inspection.unwrap().security_schemes["OAuth"]["type"],
        "oauth2"
    );
}

#[test]
fn path_item_siblings_and_reference_annotations_keep_source_provenance() {
    let document = json!({"openapi":"3.1.2","info":{"title":"Pets","version":"1"},"paths":{"/pets":{"$ref":"path.json","post":{"operationId":"create","responses":{"200":{"$ref":"response.json","description":"Own description"}}}}}});
    let mut input = request(Operation::Analyze, document);
    input.options.files.insert(
        "path.json".into(),
        json!({"get":{"operationId":"list","responses":{"200":{"description":"OK"}}}}).to_string(),
    );
    input.options.files.insert(
        "response.json".into(),
        json!({"description":"Base description"}).to_string(),
    );
    let report = run(input);
    assert!(report.valid, "{:?}", report.diagnostics);
    let inspection = report.inspection.unwrap();
    assert_eq!(inspection.operations.len(), 2);
    let get = inspection
        .operations
        .iter()
        .find(|op| op.method == "GET")
        .unwrap();
    let post = inspection
        .operations
        .iter()
        .find(|op| op.method == "POST")
        .unwrap();
    assert!(get.location.address.uri.ends_with("/path.json"));
    assert!(post.location.address.uri.ends_with("/openapi.json"));
    assert_eq!(post.responses["200"]["description"], "Own description");
}

#[test]
fn bundling_external_path_items_keeps_local_operations() {
    let document = json!({"openapi":"3.1.2","info":{"title":"Pets","version":"1"},"paths":{"/pets":{"$ref":"path.json","post":{"responses":{"200":{"description":"Created"}}}}}});
    let mut input = request(Operation::Bundle, document);
    input.options.files.insert(
        "path.json".into(),
        json!({"get":{"responses":{"200":{"description":"Listed"}}}}).to_string(),
    );
    let bundle = run(input).document.unwrap();
    assert!(bundle["paths"]["/pets"]["get"].is_object());
    assert!(bundle["paths"]["/pets"]["post"].is_object());
    let mut moved = request(Operation::Analyze, bundle);
    moved.input.uri = "memory://elsewhere/new.json".into();
    assert!(run(moved).valid);
}

#[test]
fn decimal_integer_overflow_keeps_numeric_type() {
    for integer in [
        "18446744073709551616",
        "-9223372036854775809",
        "9007199254740993",
    ] {
        for extension in ["json", "yaml"] {
            let text = format!(
                r#"{{"openapi":"3.1.2","info":{{"title":"Numbers","version":"1"}},"paths":{{}},"x-number":{integer}}}"#
            );
            let expected: Value = serde_json::from_str(&text).unwrap();
            let mut input = request(Operation::Parse, json!({}));
            input.input.uri = format!("memory://octri/numbers.{extension}");
            input.input.content = Some(text);
            let result = run(input).document.unwrap();
            assert_eq!(result["x-number"], expected["x-number"]);
            assert!(result["x-number"].is_number());
        }
    }
}

#[test]
fn schema_usage_tracks_transitive_descendants_without_prefix_collisions() {
    let document = api(
        "3.1.2",
        json!({
            "Pet":{"type":"object","properties":{"name":{"type":"string"}}},
            "Pet-extra":{"type":"string"},
            "Name":{"$ref":"#/components/schemas/Pet/properties/name"},
            "Alias":{"$ref":"#/components/schemas/Name"},
            "Other":{"$ref":"#/components/schemas/Pet-extra"}
        }),
    );
    let inspection = run(request(Operation::Inspect, document))
        .inspection
        .unwrap();
    let pet = inspection
        .schemas
        .iter()
        .find(|schema| schema.name == "Pet")
        .unwrap();
    let pointers: Vec<_> = pet
        .used_by
        .iter()
        .map(|location| location.address.pointer.as_str())
        .collect();
    assert_eq!(
        pointers,
        vec![
            "/components/schemas/Alias/$ref",
            "/components/schemas/Name/$ref"
        ]
    );
}

#[test]
fn generated_json_preserves_values_pointers_and_budgets() {
    let document = api("3.1.2", json!({"A/B~C": {"type":"number", "default":1.25}}));
    let original = run(request(Operation::Parse, document.clone()));
    let mut input = request(Operation::Parse, document.clone());
    input.input.generated = true;
    let generated = run(input.clone());
    assert_eq!(generated.document, original.document);
    assert_eq!(
        generated.sources[0].locations.keys().collect::<Vec<_>>(),
        original.sources[0].locations.keys().collect::<Vec<_>>()
    );
    assert!(
        generated.sources[0]
            .locations
            .values()
            .all(|loc| loc.line.is_none() && loc.column.is_none() && loc.offset.is_none())
    );
    for (content, code) in [
        (r#"{"openapi":"3.1.2","openapi":"3.1.2"}"#, "DUPLICATE_KEY"),
        (r#"{"__proto__":{}}"#, "UNSAFE_KEY"),
        ("openapi: 3.1.2", "INVALID_JSON"),
    ] {
        input.input.content = Some(content.into());
        let error = execute(input.clone(), &mut NoIo).unwrap_err();
        assert_eq!(error.diagnostics[0].code, code);
    }
    for (nodes, depth, bytes, code) in [
        (1, 100, 10000, "LIMIT_NODES"),
        (10000, 1, 10000, "LIMIT_DEPTH"),
        (10000, 100, 1, "LIMIT_DOCUMENT_BYTES"),
    ] {
        input.input.content = Some(document.to_string());
        input.options.limits.max_nodes = nodes;
        input.options.limits.max_depth = depth;
        input.options.limits.max_document_bytes = bytes;
        assert_eq!(
            execute(input.clone(), &mut NoIo).unwrap_err().diagnostics[0].code,
            code
        );
    }
}

#[test]
fn compatible_defaults_preserve_sources_and_strict_mode_rejects_original() {
    for version in [
        None,
        Some(json!(42)),
        Some(Value::Null),
        Some(json!(["unexpected"])),
        Some(json!(false)),
        Some(json!({"type":"string","$id":false,"$ref":"missing.yaml"})),
    ] {
        let mut document = api("3.1.2", json!({}));
        document["info"].as_object_mut().unwrap().remove("version");
        if let Some(version) = version {
            document["info"]["version"] = version;
        }
        let mut input = request(Operation::Analyze, document.clone());
        assert!(!run(input.clone()).valid);
        input.options.validate.mode = octri_parser::ValidationMode::Compatible;
        let report = run(input.clone());
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_eq!(report.document.unwrap()["info"]["version"], "0.1.0");
        assert_eq!(report.sources[0].document, document);
        assert_eq!(
            report.registry.root_document.unwrap()["info"]["version"],
            "0.1.0"
        );
        input.operation = Operation::Validate;
        let report = run(input);
        assert!(report.valid);
        assert_eq!(report.document.unwrap()["info"]["version"], "0.1.0");
        assert_eq!(report.sources[0].document, document);
    }
}

#[test]
fn compatible_accepts_generator_repairable_shapes_without_changing_constraints() {
    let fixtures: Vec<Value> =
        serde_json::from_str(include_str!("../../../fixtures/cases.json")).unwrap();
    for name in [
        "invalid-schema-keyword",
        "external-invalid-response",
        "duplicate-operation-id",
    ] {
        let fixture = fixtures
            .iter()
            .find(|fixture| fixture["name"] == name)
            .unwrap();
        let mut input = request(Operation::Analyze, fixture["input"].clone());
        input.options =
            serde_json::from_value(fixture.get("options").cloned().unwrap_or(json!({}))).unwrap();
        assert!(run(input.clone()).valid, "{name}");
        input.options.validate.mode = octri_parser::ValidationMode::Strict;
        assert!(!run(input).valid, "{name}");
    }
    let mut invalid = request(
        Operation::Analyze,
        api(
            "3.1.2",
            json!({"A":{"$ref":"#/components/schemas/Missing"}}),
        ),
    );
    invalid.options.validate.mode = octri_parser::ValidationMode::Compatible;
    assert!(!run(invalid).valid);
}

#[test]
fn json_surrogate_pairs_keep_original_text_and_precise_locations() {
    let text = "{\n  \"openapi\": \"3.1.2\",\n  \"info\": {\"title\": \"Hello! \\ud83d\\udc4d\", \"version\": \"1\"},\n  \"paths\": {},\n  \"x-escaped\": {\"a/b~c\": [\"é\", true]}\n}";
    for uri in ["memory://octri/input.json", "memory://octri/input.yaml"] {
        let mut input = request(Operation::Analyze, json!({}));
        input.input.uri = uri.into();
        input.input.content = Some(text.into());
        let report = run(input);
        assert!(report.valid, "{:?}", report.diagnostics);
        let source = &report.sources[0];
        assert_eq!(source.text, text);
        assert!(!source.generated);
        assert_eq!(source.document["info"]["title"], "Hello! 👍");
        for (pointer, token) in [
            ("/info/title", "\"Hello!"),
            ("/x-escaped/a~1b~0c/0", "\"é\""),
            ("/x-escaped/a~1b~0c/1", "true"),
        ] {
            let location = &source.locations[pointer];
            let offset = text.find(token).unwrap();
            assert_eq!(location.offset, Some(offset));
            assert_eq!(
                location.line,
                Some(text[..offset].bytes().filter(|b| *b == b'\n').count() + 1)
            );
            assert_eq!(
                location.column,
                Some(text[..offset].rsplit('\n').next().unwrap().chars().count() + 1)
            );
        }
    }
    for text in [
        r#"{"openapi":"3.1.2","info":{"title":"\ud83d","version":"1"},"paths":{}}"#,
        r#"{"openapi":"3.1.2","paths":{},"paths":{}}"#,
    ] {
        let mut input = request(Operation::Parse, json!({}));
        input.input.content = Some(text.into());
        let error = execute(input, &mut NoIo).unwrap_err();
        assert!(matches!(
            error.diagnostics[0].code.as_str(),
            "INVALID_JSON" | "DUPLICATE_KEY"
        ));
        assert!(error.diagnostics[0].location.line.is_some());
    }
    let mut yaml = request(Operation::Analyze, json!({}));
    yaml.input.uri = "memory://octri/input.yaml".into();
    yaml.input.content =
        Some("{openapi: 3.1.2, info: {title: Flow, version: '1'}, paths: {}}".into());
    assert!(run(yaml).valid);
}

#[test]
fn document_execution_keeps_limits_and_external_error_positions() {
    let input = request(
        Operation::Validate,
        api("3.1.2", json!({"A":{"type":"string"}})),
    );
    for limit in ["document", "nodes", "depth", "output"] {
        let mut bounded = input.clone();
        match limit {
            "document" => bounded.options.limits.max_document_bytes = 1,
            "nodes" => bounded.options.limits.max_nodes = 1,
            "depth" => bounded.options.limits.max_depth = 1,
            _ => bounded.options.limits.max_output_bytes = 1,
        }
        let full = execute(bounded.clone(), &mut NoIo).unwrap_err();
        let document = octri_parser::execute_document(bounded, &mut NoIo).unwrap_err();
        assert_eq!(
            serde_json::to_value(full).unwrap(),
            serde_json::to_value(document).unwrap()
        );
    }
    let mut external = request(Operation::Validate, api("3.1.2", json!({})));
    external.input.content = Some("openapi: 3.1.2\ninfo: {title: External, version: '1'}\npaths:\n  /pets:\n    get:\n      responses:\n        '200': {$ref: response.yaml}\n".into());
    external.input.uri = "memory://octri/root.yaml".into();
    external
        .options
        .files
        .insert("response.yaml".into(), "description: 42\n".into());
    let full = execute(external.clone(), &mut NoIo).unwrap_err();
    let document = octri_parser::execute_document(external, &mut NoIo).unwrap_err();
    assert_eq!(
        serde_json::to_value(full).unwrap(),
        serde_json::to_value(document).unwrap()
    );
}

#[test]
fn references_to_overlapping_parent_and_child_targets_share_graph_nodes() {
    let report = run(request(
        Operation::Dereference,
        api(
            "3.1.2",
            json!({
                "AChild": {"$ref":"#/components/schemas/Parent/properties/child"},
                "BParent": {"$ref":"#/components/schemas/Parent"},
                "CScalar": {"$ref":"#/components/schemas/Parent/properties/child/properties/id/type"},
                "Parent": {"type":"object","properties":{"child":{"type":"object","properties":{"id":{"type":"string"}}}}}
            }),
        ),
    ));
    let graph = report.graph.unwrap();
    let field = |node: usize, key: &str| match &graph.nodes[node] {
        GraphNode::Object(fields) => fields[key],
        _ => panic!("Expected object"),
    };
    let schemas = field(field(graph.root, "components"), "schemas");
    let parent = field(schemas, "Parent");
    assert_eq!(field(schemas, "BParent"), parent);
    let child = field(field(parent, "properties"), "child");
    assert_eq!(field(schemas, "AChild"), child);
    let scalar = field(field(field(child, "properties"), "id"), "type");
    assert_eq!(field(schemas, "CScalar"), scalar);
}
