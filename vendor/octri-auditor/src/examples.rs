//! Example validation reuses a bounded, offline resource registry per audit.
//! Source documents are cloned once, never retrieved from the network.

use crate::source::Sources;
use octri_parser::{Address, Dialect, Report};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub struct Examples {
    registry: Option<jsonschema::Registry<'static>>,
    registry_error: Option<String>,
    draft: jsonschema::Draft,
}

struct RefuseRetrieval;
impl jsonschema::Retrieve for RefuseRetrieval {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> std::result::Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("External example retrieval is disabled: {uri}"),
        )
        .into())
    }
}

pub enum ExampleResult {
    Valid,
    Invalid(String),
    Unsupported(String),
}

impl Examples {
    pub fn new(report: &Report) -> Self {
        let legacy = matches!(report.dialect, Some(Dialect::OpenApi30 | Dialect::Swagger2));
        let draft = if legacy {
            jsonschema::Draft::Draft4
        } else {
            jsonschema::Draft::Draft202012
        };
        let mut documents: BTreeMap<_, _> = report
            .sources
            .iter()
            .map(|source| (source.uri.clone(), source.document.clone()))
            .collect();
        if legacy {
            for at in Sources(report).schemas() {
                if let Some(schema) = documents
                    .get_mut(&at.uri)
                    .and_then(|document| document.pointer_mut(&at.pointer))
                {
                    normalize30(schema);
                }
            }
        }
        // Logical resource IDs are aliases for parser-verified physical sources.
        // URI/anchor semantics remain the parser's responsibility.
        let aliases: Vec<_> = report
            .registry
            .resources
            .iter()
            .filter_map(|(uri, at)| {
                documents
                    .get(&at.uri)
                    .and_then(|document| document.pointer(&at.pointer))
                    .map(|value| (uri.clone(), value.clone()))
            })
            .collect();
        let result = jsonschema::Registry::new()
            .draft(draft)
            .retriever(RefuseRetrieval)
            .extend(documents.into_iter().chain(aliases))
            .and_then(|builder| builder.prepare());
        match result {
            Ok(registry) => Self {
                registry: Some(registry),
                registry_error: None,
                draft,
            },
            Err(error) => Self {
                registry: None,
                registry_error: Some(error.to_string()),
                draft,
            },
        }
    }

    pub fn validate(&self, example: &Value, at: &Address) -> ExampleResult {
        let Some(registry) = &self.registry else {
            return ExampleResult::Unsupported(
                self.registry_error
                    .clone()
                    .unwrap_or_else(|| "No schema resource registry".into()),
            );
        };
        let Ok(mut uri) = url::Url::parse(&at.uri) else {
            return ExampleResult::Unsupported("Invalid schema source URI".into());
        };
        // Encode pointers as RFC 3986 fragments: WHATWG URLs permit path-template
        // braces here, but the JSON Schema URI parser rejects unescaped braces.
        let mut fragment = String::new();
        for byte in at.pointer.bytes() {
            if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
                fragment.push(char::from(byte));
            } else {
                fragment.push_str(&format!("%{byte:02X}"));
            }
        }
        uri.set_fragment(Some(&fragment));
        // Compile a reference to the registered physical schema. Compiling a
        // detached subschema under its document URI would shadow the full root
        // and break #/components references, including recursive models.
        let schema = json!({"$ref": uri.as_str()});
        let validator = jsonschema::options()
            .offline()
            .with_registry(registry)
            .with_draft(self.draft)
            .build(&schema);
        match validator {
            Ok(validator) => match validator.iter_errors(example).next() {
                Some(error) => ExampleResult::Invalid(error.to_string()),
                None => ExampleResult::Valid,
            },
            Err(error) => ExampleResult::Unsupported(error.to_string()),
        }
    }
}

fn normalize30(schema: &mut Value) {
    if let Some(map) = schema.as_object_mut() {
        if map.get("nullable") == Some(&Value::Bool(true))
            && let Some(kind) = map.get("type").and_then(Value::as_str).map(str::to_owned)
        {
            map.insert("type".into(), json!([kind, "null"]));
        }
        map.remove("nullable");
        for key in ["properties", "patternProperties", "definitions"] {
            if let Some(values) = map.get_mut(key).and_then(Value::as_object_mut) {
                for value in values.values_mut() {
                    normalize30(value);
                }
            }
        }
        for key in ["items", "additionalProperties", "not"] {
            if let Some(value) = map.get_mut(key) {
                normalize30(value);
            }
        }
        for key in ["allOf", "oneOf", "anyOf"] {
            if let Some(values) = map.get_mut(key).and_then(Value::as_array_mut) {
                for value in values {
                    normalize30(value);
                }
            }
        }
    }
}
