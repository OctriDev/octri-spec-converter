//! Resolve RAML library namespaces without activating example or annotation data.
use super::includes;
use crate::{
    ConversionError, Result,
    source::{Sources, child, hash},
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

const GROUPS: [&str; 6] = [
    "types",
    "schemas",
    "traits",
    "resourceTypes",
    "securitySchemes",
    "annotationTypes",
];
type Scope = BTreeMap<String, String>;

pub(super) fn apply(mut root: Value, sources: &mut Sources) -> Result<Value> {
    let mut libraries = Libraries {
        sources,
        exports: BTreeMap::new(),
        declarations: BTreeMap::new(),
        nodes: 0,
    };
    let scope = libraries.imports(&root, "", 0)?;
    libraries.qualify(&mut root, &scope, Position::Container, 0)?;
    for (group, declarations) in libraries.declarations {
        let target = root
            .as_object_mut()
            .ok_or_else(|| ConversionError::new("RAML_LIBRARY", "API document must be an object"))?
            .entry(group)
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| ConversionError::new("RAML_LIBRARY", "Declarations must be a map"))?;
        for (name, value) in declarations {
            if target.insert(name, value).is_some() {
                return Err(ConversionError::new(
                    "RAML_LIBRARY_COLLISION",
                    "Library declaration conflicts with an API declaration",
                ));
            }
        }
    }
    Ok(root)
}

struct Libraries<'a> {
    sources: &'a mut Sources,
    exports: BTreeMap<String, Scope>,
    declarations: BTreeMap<String, Map<String, Value>>,
    nodes: usize,
}

#[derive(Clone, Copy)]
enum Position {
    Container,
    Type,
    TypeMap,
    Body,
    Applications,
}

impl Libraries<'_> {
    fn imports(&mut self, value: &Value, pointer: &str, depth: usize) -> Result<Scope> {
        let mut scope = Scope::new();
        let Some(uses) = value.get("uses") else {
            return Ok(scope);
        };
        let uses = uses.as_object().ok_or_else(|| {
            ConversionError::new("RAML_LIBRARY", "uses must be a namespace-to-location map")
        })?;
        for (namespace, path) in uses {
            if namespace.is_empty()
                || namespace.contains('.')
                || !namespace
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-'))
            {
                return Err(ConversionError::new(
                    "RAML_LIBRARY_NAMESPACE",
                    "Library namespace must be a single RAML identifier",
                ));
            }
            let at = child(&child(pointer, "uses"), namespace);
            let physical = self.sources.origins.get(&at).ok_or_else(|| {
                ConversionError::new(
                    "RAML_LIBRARY_SOURCE",
                    "Library application has no physical source",
                )
            })?;
            let path = path.as_str().ok_or_else(|| {
                ConversionError::new("RAML_LIBRARY", "Library location must be a string")
            })?;
            let uri = self.sources.relative(&physical.uri, path)?;
            let prefix = if pointer.is_empty() {
                namespace.to_owned()
            } else {
                format!("Library_{}", &hash(uri.as_bytes())[..16])
            };
            for (name, qualified) in self.load(&uri, &prefix, depth + 1)? {
                scope.insert(format!("{namespace}.{name}"), qualified);
            }
        }
        Ok(scope)
    }

    fn load(&mut self, uri: &str, prefix: &str, depth: usize) -> Result<Scope> {
        if let Some(exports) = self.exports.get(uri) {
            return Ok(exports.clone());
        }
        if depth > self.sources.limits.max_depth {
            return Err(ConversionError::new(
                "RAML_LIBRARY_DEPTH",
                "Library graph exceeds depth limit",
            ));
        }
        let text = self.sources.text(uri)?.trim_start_matches('\u{feff}');
        if text.lines().next().map(str::trim) != Some("#%RAML 1.0 Library") {
            return Err(ConversionError::new(
                "RAML_LIBRARY_HEADER",
                "uses must reference a RAML 1.0 Library document",
            ));
        }
        let pointer = child("/_libraries", &hash(uri.as_bytes())[..16]);
        let mut library = includes::expand_uri(self.sources, uri, &pointer)?;
        super::version::normalize_type_group(&mut library, &pointer, self.sources)?;
        let mut own = Scope::new();
        for group in GROUPS {
            if let Some(values) = library.get(group) {
                let values = values.as_object().ok_or_else(|| {
                    ConversionError::new(
                        "RAML_LIBRARY_DECLARATIONS",
                        "Library declaration groups must be maps",
                    )
                })?;
                for name in values.keys() {
                    own.insert(name.clone(), format!("{prefix}.{name}"));
                }
            }
        }
        // Register exports before following dependencies so legal recursive type
        // graphs terminate without unbounded namespace composition.
        self.exports.insert(uri.into(), own.clone());
        let mut scope = own.clone();
        scope.extend(self.imports(&library, &pointer, depth)?);
        for group in GROUPS {
            let Some(values) = library.get_mut(group).and_then(Value::as_object_mut) else {
                continue;
            };
            for (name, value) in values {
                let position = if matches!(group, "types" | "schemas" | "annotationTypes") {
                    Position::Type
                } else {
                    Position::Container
                };
                self.qualify(value, &scope, position, 0)?;
                let qualified = own.get(name).ok_or_else(|| {
                    ConversionError::new("RAML_LIBRARY_EXPORT", "Library export is missing")
                })?;
                self.declarations
                    .entry(group.into())
                    .or_default()
                    .insert(qualified.clone(), value.clone());
                let from = child(&child(&pointer, group), name);
                let to = child(&child("", group), qualified);
                let origins: Vec<_> = self
                    .sources
                    .origins
                    .iter()
                    .filter_map(|(path, origin)| {
                        path.strip_prefix(&from)
                            .filter(|suffix| suffix.is_empty() || suffix.starts_with('/'))
                            .map(|suffix| (format!("{to}{suffix}"), origin.clone()))
                    })
                    .collect();
                self.sources.origins.extend(origins);
            }
        }
        Ok(own)
    }

    fn qualify(
        &mut self,
        value: &mut Value,
        scope: &Scope,
        position: Position,
        depth: usize,
    ) -> Result<()> {
        self.nodes += 1;
        if self.nodes > self.sources.limits.max_nodes || depth > self.sources.limits.max_depth {
            return Err(ConversionError::new(
                "RAML_LIBRARY_EXPANSION",
                "Library expansion exceeds node or depth limit",
            ));
        }
        if value.get("$schema").is_some() {
            return Ok(());
        }
        match value {
            Value::String(text) => {
                if matches!(position, Position::Type | Position::Body) {
                    *text = qualify_expression(text, scope);
                } else if matches!(position, Position::Applications)
                    && let Some(qualified) = scope.get(text)
                {
                    *text = qualified.clone();
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.qualify(value, scope, position, depth + 1)?;
                }
            }
            Value::Object(map) => {
                if matches!(position, Position::Applications) {
                    let original = std::mem::take(map);
                    for (key, value) in original {
                        map.insert(scope.get(&key).cloned().unwrap_or(key), value);
                    }
                    return Ok(());
                }
                let media_map =
                    matches!(position, Position::Body) && map.keys().any(|key| key.contains('/'));
                let original = std::mem::take(map);
                for (key, mut value) in original {
                    let is_annotation = key.starts_with('(') && key.ends_with(')');
                    let renamed = if is_annotation {
                        scope
                            .get(&key[1..key.len() - 1])
                            .map(|name| format!("({name})"))
                            .unwrap_or_else(|| key.clone())
                    } else {
                        key.clone()
                    };
                    if !is_annotation
                        && !matches!(
                            key.as_str(),
                            "example" | "examples" | "default" | "enum" | "settings"
                        )
                    {
                        let next = if matches!(position, Position::TypeMap) || media_map {
                            Position::Type
                        } else {
                            match key.as_str() {
                                "type" | "schema" | "items" => Position::Type,
                                "types" | "schemas" | "annotationTypes" | "properties"
                                | "facets" | "queryParameters" | "headers" | "uriParameters"
                                | "baseUriParameters" => Position::TypeMap,
                                "body" => Position::Body,
                                "is" | "securedBy" => Position::Applications,
                                _ => Position::Container,
                            }
                        };
                        self.qualify(&mut value, scope, next, depth + 1)?;
                    }
                    map.insert(renamed, value);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn qualify_expression(text: &str, scope: &Scope) -> String {
    if super::schema_use::document_text(text).starts_with(['{', '<']) {
        // Embedded JSON/XML is a document, not a RAML type expression. Names
        // inside properties, examples, identifiers and references stay literal.
        return text.into();
    }
    let mut output = String::new();
    let mut token = String::new();
    for ch in text.chars().chain(std::iter::once('\0')) {
        if ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.') {
            token.push(ch);
        } else {
            output.push_str(scope.get(&token).map_or(token.as_str(), String::as_str));
            token.clear();
            if ch != '\0' {
                output.push(ch);
            }
        }
    }
    output
}
