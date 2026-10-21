//! JSON/YAML export through bounded writers. Graphs must be converted to trees
//! first; cycles are an explicit error rather than silently discarded.

use crate::{Address, Limits, ParserError, Result};
use serde_json::Value;
use yaml_rust2::{Yaml, YamlEmitter};

/// Verify tree and encoded-byte budgets without allocating unused JSON output.
pub(crate) fn check_document(value: &Value, limits: &Limits) -> Result<()> {
    check_tree(value, limits)?;
    measure_json(value, limits.max_output_bytes)?;
    Ok(())
}

pub fn serialize(value: &Value, limits: &Limits) -> Result<String> {
    check_tree(value, limits)?;
    let mut writer = BoundedBytes {
        bytes: Vec::new(),
        limit: limits.max_output_bytes,
    };
    serde_json::to_writer(&mut writer, value).map_err(|e| output_error(e.to_string()))?;
    String::from_utf8(writer.bytes).map_err(|e| output_error(e.to_string()))
}

pub fn serialize_yaml(value: &Value, limits: &Limits) -> Result<String> {
    check_tree(value, limits)?;
    let mut writer = BoundedText {
        text: String::new(),
        limit: limits.max_output_bytes,
    };
    YamlEmitter::new(&mut writer)
        .dump(&as_yaml(value))
        .map_err(|e| output_error(e.to_string()))?;
    Ok(writer.text)
}

fn as_yaml(value: &Value) -> Yaml {
    match value {
        Value::Object(map) => Yaml::Hash(
            map.iter()
                .map(|(key, value)| (Yaml::String(key.clone()), as_yaml(value)))
                .collect(),
        ),
        Value::Array(values) => Yaml::Array(values.iter().map(as_yaml).collect()),
        Value::String(value) => Yaml::String(value.clone()),
        Value::Number(value) => value
            .as_i64()
            .map_or_else(|| Yaml::Real(value.to_string()), Yaml::Integer),
        Value::Bool(value) => Yaml::Boolean(*value),
        Value::Null => Yaml::Null,
    }
}
fn output_error(message: String) -> ParserError {
    ParserError::new("LIMIT_OUTPUT_BYTES", message, Address::default())
}
struct BoundedBytes {
    bytes: Vec<u8>,
    limit: usize,
}
impl std::io::Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            return Err(std::io::Error::other("Output byte limit exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
struct BoundedText {
    text: String,
    limit: usize,
}
impl std::fmt::Write for BoundedText {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        if self.text.len().saturating_add(text.len()) > self.limit {
            return Err(std::fmt::Error);
        }
        self.text.push_str(text);
        Ok(())
    }
}

/// Count encoded bytes without allocating a serialized copy.
pub(crate) fn measure_json(value: &impl serde::Serialize, limit: usize) -> Result<usize> {
    struct Counter {
        used: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.used = self.used.saturating_add(bytes.len());
            if self.used > self.limit {
                return Err(std::io::Error::other("Encoded byte budget exceeded"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { used: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|e| output_error(e.to_string()))?;
    Ok(counter.used)
}

fn check_tree(value: &Value, limits: &Limits) -> Result<()> {
    limits.check()?;
    let mut queue = vec![(value, 0usize)];
    let mut nodes = 0;
    while let Some((value, depth)) = queue.pop() {
        nodes += 1;
        if nodes > limits.max_nodes || depth > limits.max_depth {
            return Err(ParserError::new(
                "LIMIT_OUTPUT_NODES",
                "Output tree exceeds node/depth limits",
                Address::default(),
            ));
        }
        match value {
            Value::Object(map) => queue.extend(map.values().map(|value| (value, depth + 1))),
            Value::Array(values) => queue.extend(values.iter().map(|value| (value, depth + 1))),
            _ => {}
        }
    }
    Ok(())
}
