use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::json;

fn request(text: &str) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://fixtures/api.raml".into(),
            content: Some(text.into()),
            generated: false,
        },
        files: Default::default(),
        config: Config::default(),
    }
}

#[test]
fn raml_integer_widths_and_datetime_formats_preserve_wire_constraints()
-> Result<(), Box<dyn std::error::Error>> {
    let text = "#%RAML 1.0\ntitle: Formats\nversion: v1\nbaseUri: https://api.example.com\ntypes:\n  Byte:\n    type: integer\n    format: int8\n    minimum: -10\n  Short:\n    type: integer\n    format: int16\n  Clock:\n    type: datetime\n    format: rfc3339\n  HttpDate:\n    type: datetime\n    format: rfc2616\n/values:\n  get:\n    responses:\n      204:\n        description: Empty\n";
    let report = convert(&request(text))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert!(
        report
            .decisions
            .iter()
            .any(|decision| decision.code == "RAML_HTTP_DATE" && decision.review_required)
    );
    let doc = report.document.ok_or("missing output")?;
    let byte = jsonschema::validator_for(&doc["components"]["schemas"]["Byte"])?;
    assert!(byte.is_valid(&json!(127)));
    assert!(!byte.is_valid(&json!(128)));
    assert!(!byte.is_valid(&json!(-11)));
    assert!(!byte.is_valid(&json!(1.5)));
    let short = jsonschema::validator_for(&doc["components"]["schemas"]["Short"])?;
    assert!(short.is_valid(&json!(-32768)));
    assert!(!short.is_valid(&json!(32768)));
    assert_eq!(doc["components"]["schemas"]["Clock"]["format"], "date-time");
    let date = jsonschema::validator_for(&doc["components"]["schemas"]["HttpDate"])?;
    for value in [
        "Sun, 06 Nov 1994 08:49:37 GMT",
        "Sunday, 06-Nov-94 08:49:37 GMT",
        "Sun Nov  6 08:49:37 1994",
    ] {
        assert!(date.is_valid(&json!(value)), "{value}");
    }
    assert!(!date.is_valid(&json!("1994-11-06T08:49:37Z")));
    Ok(())
}

#[test]
fn raml_multifile_types_traits_and_nested_paths_preserve_constraints()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "#%RAML 1.0\ntitle: Pets\nversion: v1\nbaseUri: https://api.example.com/{version}\nmediaType: application/json\ntypes:\n  Pet: !include types/pet.raml\ntraits:\n  paged:\n    queryParameters:\n      limit?:\n        type: integer\n        minimum: 1\n        maximum: 100\n/pets:\n  get:\n    is: [paged]\n    responses:\n      200:\n        body: Pet[]\n  /{id}:\n    uriParameters:\n      id: string\n    get:\n      responses:\n        200:\n          body: Pet\n";
    let mut input = request(source);
    input.files.insert("types/pet.raml".into(),"#%RAML 1.0 DataType\ntype: object\nproperties:\n  id: integer\n  nickname?: string | nil\nexample:\n  id: 42\n".into());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let doc = report.document.ok_or("missing output")?;
    assert_eq!(
        doc.pointer("/components/schemas/Pet/required"),
        Some(&json!(["id"]))
    );
    assert_eq!(
        doc.pointer("/paths/~1pets/get/parameters/0/required"),
        Some(&json!(false))
    );
    assert_eq!(
        doc.pointer("/paths/~1pets/get/parameters/0/schema/maximum"),
        Some(&json!(100))
    );
    assert_eq!(
        doc.pointer("/paths/~1pets~1{id}/get/parameters/0/in"),
        Some(&json!("path"))
    );
    assert_eq!(
        doc.pointer("/servers/0/url"),
        Some(&json!("https://api.example.com/v1"))
    );
    assert!(report.decisions.iter().any(|decision| {
        decision.source.address.uri.ends_with("types/pet.raml") && decision.source.line.is_some()
    }));
    Ok(())
}

#[test]
fn raml_includes_cannot_read_ambient_files_and_cycles_fail() {
    let source = "#%RAML 1.0\ntitle: Pets\ntypes:\n  Pet: !include /etc/passwd\n";
    assert!(convert(&request(source)).is_err());
    let mut input = request("#%RAML 1.0\ntitle: Pets\ntypes:\n  Pet: !include pet.raml\n");
    input.files.insert(
        "pet.raml".into(),
        "#%RAML 1.0 DataType\ntype: !include pet.raml\n".into(),
    );
    assert!(convert(&input).is_err());
}

#[test]
fn raml_optional_resource_methods_do_not_create_operations()
-> Result<(), Box<dyn std::error::Error>> {
    let text = "#%RAML 1.0\ntitle: Pets\nversion: v1\nbaseUri: https://api.example.com\nresourceTypes:\n  collection:\n    get?:\n      responses:\n        204:\n          description: Empty\n    post?:\n      responses:\n        201:\n          description: Created\n/pets:\n  type: collection\n  get:\n    description: List pets\n";
    let report = convert(&request(text))?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let doc = report.document.ok_or("missing output")?;
    assert!(doc.pointer("/paths/~1pets/get/responses/204").is_some());
    assert!(doc.pointer("/paths/~1pets/post").is_none());
    Ok(())
}

#[test]
fn raml_json_schema_includes_keep_local_resource_scope() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(
        "#%RAML 1.0\ntitle: Trees\nversion: v1\nbaseUri: https://api.example.com\nmediaType: application/json\ntypes:\n  Tree: !include schemas/tree.json\n/trees:\n  post:\n    body: Tree\n    responses:\n      202:\n        description: Accepted\n",
    );
    input.files.insert("schemas/tree.json".into(),json!({"$schema":"https://json-schema.org/draft/2020-12/schema","$defs":{"Tree":{"$dynamicAnchor":"tree","type":"object","properties":{"child":{"$dynamicRef":"#tree"}}}},"$ref":"#/$defs/Tree"}).to_string());
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("missing output")?;
    assert!(document.pointer("/components/schemas/Tree/$ref").is_some());
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
fn raml_libraries_resolve_scoped_types_traits_and_transitive_dependencies()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(
        "#%RAML 1.0\ntitle: Pets\nversion: v1\nbaseUri: https://api.example.com\nmediaType: application/json\nuses:\n  pets: libraries/pets.raml\ntypes:\n  Pet: pets.Pet\n/pets:\n  get:\n    is: [pets.paged]\n    responses:\n      200:\n        body: Pet[]\n",
    );
    input.files.insert("libraries/pets.raml".into(), "#%RAML 1.0 Library\nuses:\n  ids: ids.raml\ntypes:\n  Pet:\n    properties:\n      id: ids.Id\n      parent?: Pet\n    example:\n      type: Pet\ntraits:\n  paged:\n    queryParameters:\n      page?: integer\n".into());
    input.files.insert(
        "libraries/ids.raml".into(),
        "#%RAML 1.0 Library\ntypes:\n  Id:\n    type: integer\n    minimum: 1\n".into(),
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let document = report.document.ok_or("missing output")?;
    assert_eq!(
        document.pointer("/components/schemas/Pet/$ref"),
        Some(&json!("#/components/schemas/pets.Pet"))
    );
    assert_eq!(
        document.pointer("/components/schemas/pets.Pet/properties/parent/$ref"),
        Some(&json!("#/components/schemas/pets.Pet"))
    );
    assert_eq!(
        document.pointer("/components/schemas/pets.Pet/example/type"),
        Some(&json!("Pet"))
    );
    assert_eq!(
        document.pointer("/paths/~1pets/get/parameters/0/name"),
        Some(&json!("page"))
    );
    assert!(report.decisions.iter().any(|decision| {
        decision.source.address.uri.ends_with("libraries/pets.raml")
            && decision.source.line.is_some()
    }));
    // Namespaces belong to one source file; composing another library's alias
    // from the API document must fail rather than expose private dependencies.
    let mut chained = input;
    chained.input.content = chained
        .input
        .content
        .map(|text| text.replace("Pet: pets.Pet", "Pet: pets.ids.Id"));
    assert!(convert(&chained).is_err());
    Ok(())
}

#[test]
fn raml_templates_apply_all_transforms_and_reserved_path_rules()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        ("!singularize", "UserRecord"),
        ("!pluralize", "UserRecords"),
        ("!uppercase", "USERRECORDS"),
        ("!lowercase", "userrecords"),
        ("!lowercamelcase", "userRecords"),
        ("!uppercamelcase", "UserRecords"),
        ("!lowerunderscorecase", "user_records"),
        ("!upperunderscorecase", "USER_RECORDS"),
        ("!lowerhyphencase", "user-records"),
        ("!upperhyphencase", "USER-RECORDS"),
    ];
    for (transform, expected) in cases {
        let text = format!(
            "#%RAML 1.0\ntitle: Users\nversion: v1\nbaseUri: https://api.example.com\ntraits:\n  named:\n    description: '<<methodName>> <<resourcePathName | {transform}>>'\n/UserRecords/{{id}}:\n  uriParameters:\n    id: string\n  get:\n    is: [named]\n    responses:\n      204:\n        description: Empty\n"
        );
        let report = convert(&request(&text))?;
        assert!(report.valid, "{transform}: {:?}", report.diagnostics);
        assert_eq!(
            report
                .document
                .ok_or("missing output")?
                .pointer("/paths/~1UserRecords~1{id}/get/description"),
            Some(&json!(format!("get {expected}"))),
            "{transform}"
        );
    }
    Ok(())
}

#[test]
fn raml_named_example_optional_annotations_do_not_emit_null_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let input = request(
        "#%RAML 1.0\ntitle: Pets\nversion: v1\nbaseUri: https://api.example.com\nmediaType: application/json\n/pets:\n  get:\n    responses:\n      200:\n        body:\n          type: string\n          examples:\n            name:\n              value: Spot\n",
    );
    let report = convert(&input)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    let document = report.document.ok_or("missing output")?;
    assert_eq!(
        document.pointer("/paths/~1pets/get/responses/200/content/application~1json/examples/name"),
        Some(&json!({"value":"Spot"}))
    );
    Ok(())
}
