//! Physical source lookup. Reference following is bounded and never expands schemas.

use octri_parser::{Address, Location, Report};
use serde_json::Value;
use std::collections::BTreeSet;

pub fn child(pointer: &str, name: &str) -> String {
    format!("{pointer}/{}", name.replace('~', "~0").replace('/', "~1"))
}

pub struct Sources<'a>(pub &'a Report);

impl Sources<'_> {
    pub fn value(&self, at: &Address) -> Option<&Value> {
        self.0
            .sources
            .iter()
            .find(|source| source.uri == at.uri)?
            .document
            .pointer(&at.pointer)
    }

    pub fn location(&self, at: &Address) -> Location {
        self.0
            .sources
            .iter()
            .find(|source| source.uri == at.uri)
            .map_or_else(|| at.clone().into(), |source| source.locate(&at.pointer))
    }

    pub fn field(&self, at: &Address, name: &str) -> Address {
        Address {
            uri: at.uri.clone(),
            pointer: child(&at.pointer, name),
        }
    }

    pub fn follow(&self, at: &Address) -> Address {
        let mut at = at.clone();
        let mut seen = BTreeSet::new();
        while seen.len() < 128 && seen.insert(at.clone()) {
            let Some(edge) = self.0.references.iter().find(|edge| {
                edge.from.address.uri == at.uri
                    && edge.from.address.pointer == child(&at.pointer, "$ref")
            }) else {
                break;
            };
            let Some(target) = &edge.target else {
                break;
            };
            at = target.address.clone();
        }
        at
    }

    /// Find the actual inherited/overridden parameter, including reusable refs.
    pub fn parameter(&self, operation: &Location, name: &str, place: &str) -> Location {
        let operation_at = &operation.address;
        let parent = operation_at
            .pointer
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent);
        let parent = parent
            .strip_suffix("/additionalOperations")
            .unwrap_or(parent);
        for base in [
            operation_at.clone(),
            Address {
                uri: operation_at.uri.clone(),
                pointer: parent.into(),
            },
        ] {
            let parameters = self.field(&base, "parameters");
            if let Some(values) = self.value(&parameters).and_then(Value::as_array) {
                for index in (0..values.len()).rev() {
                    let at = self.follow(&self.field(&parameters, &index.to_string()));
                    if self
                        .value(&at)
                        .is_some_and(|value| value["name"] == name && value["in"] == place)
                    {
                        return self.location(&at);
                    }
                }
            }
        }
        operation.clone()
    }

    pub fn schemas(&self) -> Vec<Address> {
        let mut found = BTreeSet::new();
        let mut visited = BTreeSet::new();
        for source in &self.0.sources {
            let root = Address {
                uri: source.uri.clone(),
                pointer: String::new(),
            };
            if source.document.get("openapi").is_some() || source.document.get("swagger").is_some()
            {
                self.api_schemas(&root, &mut found, &mut visited, 0);
            }
        }
        for edge in &self.0.references {
            if edge.kind == "schema"
                && let Some(target) = &edge.target
            {
                self.schema(&target.address, &mut found, 0);
            }
        }
        found.into_iter().collect()
    }

    fn api_schemas(
        &self,
        at: &Address,
        found: &mut BTreeSet<Address>,
        visited: &mut BTreeSet<Address>,
        depth: usize,
    ) {
        if depth > 128 || !visited.insert(at.clone()) {
            return;
        }
        let target = self.follow(at);
        if target != *at {
            self.api_schemas(&target, found, visited, depth + 1);
        }
        let Some(value) = self.value(at).and_then(Value::as_object) else {
            return;
        };
        // Only API structural slots are traversed. Examples, extensions and
        // arbitrary defaults are data, even if their keys look like schemas.
        for (name, item) in value {
            let next = self.field(at, name);
            match name.as_str() {
                "schema" | "itemSchema" => self.schema(&next, found, 0),
                "schemas" | "definitions"
                    if at.pointer == "/components" || at.pointer.is_empty() =>
                {
                    if let Some(map) = item.as_object() {
                        for name in map.keys() {
                            self.schema(&self.field(&next, name), found, 0);
                        }
                    }
                }
                "paths"
                | "webhooks"
                | "callbacks"
                | "responses"
                | "content"
                | "headers"
                | "requestBodies"
                | "pathItems"
                | "securitySchemes"
                | "mediaTypes"
                | "additionalOperations" => {
                    if let Some(map) = item.as_object() {
                        for name in map.keys() {
                            self.api_schemas(&self.field(&next, name), found, visited, depth + 1);
                        }
                    }
                }
                "parameters" => {
                    if let Some(values) = item.as_array() {
                        for index in 0..values.len() {
                            self.api_schemas(
                                &self.field(&next, &index.to_string()),
                                found,
                                visited,
                                depth + 1,
                            );
                        }
                    } else if let Some(map) = item.as_object() {
                        for name in map.keys() {
                            self.api_schemas(&self.field(&next, name), found, visited, depth + 1);
                        }
                    }
                }
                "components" | "requestBody" | "get" | "put" | "post" | "delete" | "patch"
                | "head" | "options" | "trace" | "query" => {
                    self.api_schemas(&next, found, visited, depth + 1);
                }
                _ if at.pointer.contains("/callbacks/") && name.starts_with("{") => {
                    self.api_schemas(&next, found, visited, depth + 1);
                }
                _ => {}
            }
        }
    }

    fn schema(&self, at: &Address, found: &mut BTreeSet<Address>, depth: usize) {
        if depth > 128 || !found.insert(at.clone()) {
            return;
        }
        let resolved = self.follow(at);
        if resolved != *at {
            self.schema(&resolved, found, depth + 1);
        }
        let Some(value) = self.value(at).and_then(Value::as_object) else {
            return;
        };
        for (key, value) in value {
            let next = self.field(at, key);
            match key.as_str() {
                "properties" | "patternProperties" | "dependentSchemas" | "$defs"
                | "definitions" => {
                    if let Some(map) = value.as_object() {
                        for key in map.keys() {
                            self.schema(&self.field(&next, key), found, depth + 1);
                        }
                    }
                }
                "allOf" | "oneOf" | "anyOf" | "prefixItems" => {
                    if let Some(values) = value.as_array() {
                        for index in 0..values.len() {
                            self.schema(&self.field(&next, &index.to_string()), found, depth + 1);
                        }
                    }
                }
                "items"
                | "additionalProperties"
                | "not"
                | "if"
                | "then"
                | "else"
                | "contains"
                | "propertyNames"
                | "unevaluatedProperties"
                | "unevaluatedItems"
                | "contentSchema" => {
                    self.schema(&next, found, depth + 1);
                }
                _ => {}
            }
        }
    }
}
