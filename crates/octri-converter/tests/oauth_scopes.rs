//! OAuth scope requirements must survive observation conversion and inheritance.
use octri_converter::{Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde_json::{Value, json};

fn request(source: &Value) -> ConversionRequest {
    ConversionRequest {
        input: Input {
            uri: "memory://fixtures/collection.json".into(),
            content: Some(source.to_string()),
            generated: false,
        },
        config: Config::default(),
        files: Default::default(),
    }
}

fn assert_scopes(document: &Value, path: &str, expected: &[&str]) {
    let requirements = &document["paths"][path]["get"]["security"];
    assert_eq!(requirements.as_array().map(Vec::len), Some(1));
    let requirement = requirements[0].as_object();
    assert_eq!(requirement.map(serde_json::Map::len), Some(1));
    for (name, scopes) in requirement.into_iter().flatten() {
        assert_eq!(scopes, &json!(expected), "{path}: operation requirement");
        let available = &document["components"]["securitySchemes"][name]["flows"]["clientCredentials"]
            ["scopes"];
        assert_eq!(
            available.as_object().map(serde_json::Map::len),
            Some(expected.len())
        );
        for scope in expected {
            assert_eq!(available.get(*scope), Some(&json!("")));
        }
    }
}

fn postman_auth(scopes: &str) -> Value {
    json!({"type":"oauth2","oauth2":[
        {"key":"grant_type","value":"client_credentials"},
        {"key":"accessTokenUrl","value":"https://auth.example.com/token"},
        {"key":"scope","value":scopes}
    ]})
}

#[test]
fn postman_scopes_expand_deduplicate_inherit_and_override() -> Result<(), Box<dyn std::error::Error>>
{
    for version in ["2.0.0", "2.1.0"] {
        let source = json!({
            "info":{"name":"Scopes","schema":format!("https://schema.getpostman.com/json/collection/v{version}/collection.json")},
            "variable":[{"key":"permissions","value":"pets:write pets:read pets:write"}],
            "auth":postman_auth("{{permissions}}"),
            "item":[
                {"name":"Inherited","request":{"method":"GET","url":"https://api.example.com/inherited"}},
                {"name":"Null inheritance","auth":null,"item":[{"name":"Inherited through null","request":{"method":"GET","url":"https://api.example.com/null-inherit","auth":null}}]},
                {"name":"Override","request":{"method":"GET","url":"https://api.example.com/override","auth":postman_auth("pets:delete")}},
                {"name":"Anonymous","request":{"method":"GET","url":"https://api.example.com/anonymous","auth":{"type":"noauth"}}}
            ]
        });
        let mut input = request(&source);
        let report = convert(&input)?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_ne!(report.status, Status::Blocked);
        let document = report.document.ok_or("missing output")?;
        assert_scopes(&document, "/inherited", &["pets:write", "pets:read"]);
        assert_scopes(&document, "/null-inherit", &["pets:write", "pets:read"]);
        assert_scopes(&document, "/override", &["pets:delete"]);
        assert_eq!(
            document["paths"]["/anonymous"]["get"]["security"],
            json!([])
        );

        let mut unresolved = source;
        unresolved["variable"] = json!([]);
        input.input.content = Some(unresolved.to_string());
        let blocked = convert(&input)?;
        assert_eq!(blocked.status, Status::Blocked);
        assert!(
            blocked
                .decisions
                .iter()
                .any(|decision| decision.code == "VARIABLE_UNRESOLVED"
                    && decision.source.address.pointer == "/auth/oauth2/2/value")
        );
    }
    Ok(())
}

fn insomnia_auth(scopes: &str) -> Value {
    json!({"type":"oauth2","grantType":"client_credentials","accessTokenUrl":"https://auth.example.com/token","scope":scopes})
}

#[test]
fn insomnia_scopes_expand_deduplicate_inherit_and_override()
-> Result<(), Box<dyn std::error::Error>> {
    let inherited = insomnia_auth("{{permissions}}");
    let overridden = insomnia_auth("pets:delete");
    let v4 = json!({"_type":"export","__export_format":4,"resources":[
        {"_id":"workspace","_type":"workspace","name":"Scopes"},
        {"_id":"group","_type":"request_group","parentId":"workspace","name":"Pets","authentication":inherited},
        {"_id":"one","_type":"request","parentId":"group","name":"Inherited","method":"GET","url":"https://api.example.com/inherited"},
        {"_id":"two","_type":"request","parentId":"group","name":"Override","method":"GET","url":"https://api.example.com/override","authentication":overridden},
        {"_id":"three","_type":"request","parentId":"group","name":"Anonymous","method":"GET","url":"https://api.example.com/anonymous","authentication":{"type":"none"}}
    ]});
    let v5 = json!({"type":"collection.insomnia.rest/5.0","name":"Scopes","collection":[{
        "name":"Pets","authentication":inherited,"children":[
            {"name":"Inherited","method":"GET","url":"https://api.example.com/inherited"},
            {"name":"Override","method":"GET","url":"https://api.example.com/override","authentication":overridden},
            {"name":"Anonymous","method":"GET","url":"https://api.example.com/anonymous","authentication":{"type":"none"}}
        ]
    }]});
    for source in [v4, v5] {
        let mut input = request(&source);
        input.config.variables.insert(
            "permissions".into(),
            "pets:write pets:read pets:write".into(),
        );
        let report = convert(&input)?;
        assert!(report.valid, "{:?}", report.diagnostics);
        assert_ne!(report.status, Status::Blocked);
        let document = report.document.ok_or("missing output")?;
        assert_scopes(&document, "/inherited", &["pets:write", "pets:read"]);
        assert_scopes(&document, "/override", &["pets:delete"]);
        assert_eq!(
            document["paths"]["/anonymous"]["get"]["security"],
            json!([])
        );
    }
    Ok(())
}
