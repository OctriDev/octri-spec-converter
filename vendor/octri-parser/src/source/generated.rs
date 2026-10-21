//! Object inputs arrive as JSON. Read them directly, enforcing budgets before
//! descending into children; these inputs have pointers but no text positions.
use crate::pointer::child;
use crate::{Address, Limits, Location, ParserError, Result, SourceDocument};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::collections::BTreeMap;
use std::fmt;

pub(crate) fn parse(uri: &str, text: String, limits: &Limits) -> Result<SourceDocument> {
    read(uri, text, limits, true, true)
}

pub(super) fn parse_text(
    uri: &str,
    text: String,
    limits: &Limits,
    locations: bool,
) -> Result<SourceDocument> {
    read(uri, text, limits, false, locations)
}

pub(crate) fn parse_document(uri: &str, text: String, limits: &Limits) -> Result<SourceDocument> {
    read(uri, text, limits, true, false)
}

fn read(
    uri: &str,
    text: String,
    limits: &Limits,
    generated: bool,
    locations: bool,
) -> Result<SourceDocument> {
    let mut state = State {
        uri,
        limits,
        nodes: 0,
        locations: BTreeMap::new(),
        failure: None,
        capture_locations: locations,
    };
    if text.len() > limits.max_document_bytes {
        return Err(state.error("LIMIT_DOCUMENT_BYTES", "Document exceeds byte limit", ""));
    }
    crate::wire::check_json_keys(uri, &text)?;
    // A UTF-8 signature is transport metadata. Keep it in the immutable source,
    // but exclude it from the JSON grammar for authored text only.
    let json = if generated {
        text.as_str()
    } else {
        text.strip_prefix('\u{feff}').unwrap_or(&text)
    };
    let has_bom = json.len() != text.len();
    let mut decoder = serde_json::Deserializer::from_str(json);
    // The visitor checks the configured depth before reading each value.
    decoder.disable_recursion_limit();
    let result = ValueSeed {
        state: &mut state,
        pointer: String::new(),
        depth: 0,
    }
    .deserialize(&mut decoder)
    .and_then(|value| decoder.end().map(|()| value));
    let document = match result {
        Ok(value) => value,
        Err(error) => {
            let mut failure = state
                .failure
                .take()
                .unwrap_or_else(|| state.error("INVALID_JSON", error.to_string(), ""));
            if !generated {
                failure.diagnostics[0].location.line = Some(error.line());
                failure.diagnostics[0].location.column =
                    Some(error.column() + usize::from(has_bom && error.line() == 1));
            }
            return Err(failure);
        }
    };
    Ok(SourceDocument {
        uri: uri.into(),
        bytes: text.len(),
        text,
        document,
        locations: state.locations,
        generated,
    })
}

struct State<'a> {
    uri: &'a str,
    limits: &'a Limits,
    nodes: usize,
    locations: BTreeMap<String, Location>,
    failure: Option<ParserError>,
    capture_locations: bool,
}
impl State<'_> {
    fn error(&self, code: &str, message: impl Into<String>, pointer: &str) -> ParserError {
        ParserError::new(
            code,
            message,
            Address {
                uri: self.uri.into(),
                pointer: pointer.into(),
            },
        )
    }
    fn reject<E: de::Error>(&mut self, code: &str, message: impl Into<String>, pointer: &str) -> E {
        let message = message.into();
        self.failure = Some(self.error(code, message.clone(), pointer));
        E::custom(message)
    }
}
struct ValueSeed<'a, 'b> {
    state: &'a mut State<'b>,
    pointer: String,
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for ValueSeed<'_, '_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<Value, D::Error> {
        if self.depth > self.state.limits.max_depth {
            return Err(self.state.reject(
                "LIMIT_DEPTH",
                "Document exceeds nesting limit",
                &self.pointer,
            ));
        }
        self.state.nodes += 1;
        if self.state.nodes > self.state.limits.max_nodes {
            return Err(self.state.reject(
                "LIMIT_NODES",
                "Document exceeds node limit",
                &self.pointer,
            ));
        }
        if self.state.capture_locations {
            self.state.locations.insert(
                self.pointer.clone(),
                Location {
                    address: Address {
                        uri: self.state.uri.into(),
                        pointer: self.pointer.clone(),
                    },
                    line: None,
                    column: None,
                    offset: None,
                },
            );
        }
        decoder.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for ValueSeed<'_, '_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Value, E> {
        Ok(value.into())
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Value, E> {
        Ok(value.into())
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("Non-finite JSON number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(value.into()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> std::result::Result<Value, E> {
        Ok(Value::String(value))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> std::result::Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(ValueSeed {
            state: self.state,
            pointer: child(&self.pointer, &values.len().to_string()),
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut values = Map::new();
        let mut next = map.next_key::<String>()?;
        if next.as_deref() == Some(crate::wire::RESERVED_NUMBER_KEY) {
            // Literal occurrences were rejected before deserialization. This
            // is serde_json's internal representation of one exact numeric token.
            let token: String = map.next_value()?;
            let number: Number = token.parse().map_err(de::Error::custom)?;
            if map.next_key::<String>()?.is_some() {
                return Err(de::Error::custom("Invalid numeric transport"));
            }
            return Ok(Value::Number(crate::numbers::canonical(&number)));
        }
        while let Some(key) = next {
            let pointer = child(&self.pointer, &key);
            if key == "__proto__" {
                return Err(self
                    .state
                    .reject("UNSAFE_KEY", "Reserved __proto__ key", &pointer));
            }
            if values.contains_key(&key) {
                return Err(self.state.reject(
                    "DUPLICATE_KEY",
                    format!("Duplicate key {key}"),
                    &pointer,
                ));
            }
            let value = map.next_value_seed(ValueSeed {
                state: self.state,
                pointer,
                depth: self.depth + 1,
            })?;
            values.insert(key, value);
            next = map.next_key::<String>()?;
        }
        Ok(Value::Object(values))
    }
}
