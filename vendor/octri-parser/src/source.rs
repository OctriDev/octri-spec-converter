//! Strict JSON/YAML loading with bounded aliases and source locations.

mod generated;
mod json;
pub(crate) use generated::parse as parse_generated;
pub(crate) use generated::parse_document as parse_generated_document;

use crate::pointer::child;
use crate::{
    Address, Dialect, Limits, Location, ParserError, Result, SourceDocument, SourceFormat,
    SyntaxDocument,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use yaml_rust2::{
    parser::{Event, Parser, Tag},
    scanner::{Marker, TScalarStyle},
};

pub(crate) fn parse(uri: &str, text: String, limits: &Limits) -> Result<SourceDocument> {
    read(uri, text, limits, true)
}

pub(crate) fn parse_document(uri: &str, text: String, limits: &Limits) -> Result<SourceDocument> {
    read(uri, text, limits, false)
}

fn read(uri: &str, text: String, limits: &Limits, locations: bool) -> Result<SourceDocument> {
    read_with_format(uri, text, limits, locations, SourceFormat::Auto)
}

pub(crate) fn read_with_format(
    uri: &str,
    text: String,
    limits: &Limits,
    locations: bool,
    format: SourceFormat,
) -> Result<SourceDocument> {
    read_syntax(uri, text, limits, locations, false, format).map(|parsed| parsed.source)
}

pub(crate) fn parse_raml(uri: &str, text: String, limits: &Limits) -> Result<SyntaxDocument> {
    read_syntax(uri, text, limits, true, true, SourceFormat::Auto)
}

fn read_syntax(
    uri: &str,
    text: String,
    limits: &Limits,
    locations: bool,
    raml: bool,
    format: SourceFormat,
) -> Result<SyntaxDocument> {
    let address = Address {
        uri: uri.into(),
        pointer: String::new(),
    };
    if text.len() > limits.max_document_bytes {
        return Err(ParserError::new(
            "LIMIT_DOCUMENT_BYTES",
            "Document exceeds byte limit",
            address,
        ));
    }
    // JSON needs its own decoder: valid UTF-16 surrogate escapes are not valid
    // YAML escapes. Keep text positions in the original, unmodified input.
    if format == SourceFormat::Json
        || (format == SourceFormat::Auto
            && url::Url::parse(uri).is_ok_and(|u| u.path().ends_with(".json")))
    {
        return json::parse(uri, text, limits, locations).map(|source| SyntaxDocument {
            source,
            includes: BTreeMap::new(),
        });
    }
    let content = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if format != SourceFormat::Yaml
        && matches!(content.trim_start().as_bytes().first(), Some(b'{' | b'['))
    {
        match json::parse(uri, text.clone(), limits, locations) {
            Ok(source) => {
                return Ok(SyntaxDocument {
                    source,
                    includes: BTreeMap::new(),
                });
            }
            Err(error) if error.diagnostics.iter().any(|d| d.code == "INVALID_JSON") => {}
            Err(error) => return Err(error),
        }
    }
    let mut reader = Reader {
        parser: Parser::new_from_str(&text),
        uri,
        limits,
        nodes: 0,
        aliases: 0,
        copied_bytes: 0,
        anchors: BTreeMap::new(),
        locations: BTreeMap::new(),
        capture_locations: locations,
        raml,
        includes: BTreeMap::new(),
        anchor_includes: BTreeMap::new(),
    };
    let document = loop {
        let (event, mark) = reader.event("")?;
        match event {
            Event::StreamStart | Event::DocumentStart => continue,
            Event::StreamEnd => {
                return Err(reader.error("EMPTY_DOCUMENT", "Document is empty", "", mark));
            }
            event => break reader.value(event, mark, "", 0)?,
        }
    };
    loop {
        let (event, mark) = reader.event("")?;
        match event {
            Event::DocumentEnd => {}
            Event::StreamEnd => break,
            _ => {
                return Err(reader.error(
                    "MULTIPLE_DOCUMENTS",
                    "Expected one YAML document",
                    "",
                    mark,
                ));
            }
        }
    }
    let locations = std::mem::take(&mut reader.locations);
    let includes = std::mem::take(&mut reader.includes);
    drop(reader);
    Ok(SyntaxDocument {
        source: SourceDocument {
            uri: uri.into(),
            bytes: text.len(),
            text,
            document,
            locations,
            generated: false,
        },
        includes,
    })
}

struct Reader<'a> {
    parser: Parser<std::str::Chars<'a>>,
    uri: &'a str,
    limits: &'a Limits,
    nodes: usize,
    aliases: usize,
    copied_bytes: usize,
    anchors: BTreeMap<usize, Value>,
    locations: BTreeMap<String, Location>,
    capture_locations: bool,
    raml: bool,
    includes: BTreeMap<String, String>,
    anchor_includes: BTreeMap<usize, BTreeMap<String, String>>,
}

impl Reader<'_> {
    fn error(
        &self,
        code: &str,
        message: impl Into<String>,
        pointer: &str,
        mark: Marker,
    ) -> ParserError {
        let mut error = ParserError::new(
            code,
            message,
            Address {
                uri: self.uri.into(),
                pointer: pointer.into(),
            },
        );
        error.diagnostics[0].location.line = Some(mark.line());
        error.diagnostics[0].location.column = Some(mark.col() + 1);
        error.diagnostics[0].location.offset = Some(mark.index());
        error
    }

    fn event(&mut self, pointer: &str) -> Result<(Event, Marker)> {
        self.parser
            .next_token()
            .map_err(|e| self.error("INVALID_YAML", e.to_string(), pointer, *e.marker()))
    }

    fn value(&mut self, event: Event, mark: Marker, pointer: &str, depth: usize) -> Result<Value> {
        if depth > self.limits.max_depth {
            return Err(self.error(
                "LIMIT_DEPTH",
                "Document exceeds nesting limit",
                pointer,
                mark,
            ));
        }
        self.nodes += 1;
        if self.nodes > self.limits.max_nodes {
            return Err(self.error("LIMIT_NODES", "Document exceeds node limit", pointer, mark));
        }
        if self.capture_locations {
            self.locations.insert(
                pointer.into(),
                Location {
                    address: Address {
                        uri: self.uri.into(),
                        pointer: pointer.into(),
                    },
                    line: Some(mark.line()),
                    column: Some(mark.col() + 1),
                    offset: Some(mark.index()),
                },
            );
        }
        let (value, anchor) = match event {
            Event::Scalar(text, style, anchor, tag) => {
                let include = self.raml
                    && tag
                        .as_ref()
                        .is_some_and(|tag| tag.handle == "!" && tag.suffix == "include");
                if include {
                    self.includes.insert(pointer.into(), text.clone());
                } else {
                    self.check_tag(tag.as_ref(), pointer, mark)?;
                }
                let value = if include
                    || tag.as_ref().is_some_and(|t| t.suffix == "str")
                    || style != TScalarStyle::Plain
                {
                    Value::String(text)
                } else {
                    scalar(&text)
                        .map_err(|message| self.error("NON_JSON_VALUE", message, pointer, mark))?
                };
                (value, anchor)
            }
            Event::MappingStart(anchor, tag) => {
                self.check_tag(tag.as_ref(), pointer, mark)?;
                let mut map = Map::new();
                loop {
                    let (key, key_mark) = self.event(pointer)?;
                    if key == Event::MappingEnd {
                        break;
                    }
                    let Event::Scalar(key, _, _, tag) = key else {
                        return Err(self.error(
                            "NON_STRING_KEY",
                            "Mapping keys must be scalar strings",
                            pointer,
                            key_mark,
                        ));
                    };
                    self.check_tag(tag.as_ref(), pointer, key_mark)?;
                    let at = child(pointer, &key);
                    if key == "__proto__" {
                        return Err(self.error(
                            "UNSAFE_KEY",
                            "Reserved __proto__ key",
                            &at,
                            key_mark,
                        ));
                    }
                    if key == crate::wire::RESERVED_NUMBER_KEY {
                        return Err(self.error("UNSAFE_KEY", "Reserved JSON numeric transport key; rename this field before conversion", &at, key_mark));
                    }
                    if key == "<<" {
                        return Err(self.error(
                            "YAML_MERGE_KEY",
                            "YAML merge keys are not supported; use explicit properties",
                            &at,
                            key_mark,
                        ));
                    }
                    if map.contains_key(&key) {
                        return Err(self.error(
                            "DUPLICATE_KEY",
                            format!("Duplicate key {key}"),
                            &at,
                            key_mark,
                        ));
                    }
                    let (event, mark) = self.event(&at)?;
                    map.insert(key, self.value(event, mark, &at, depth + 1)?);
                }
                (Value::Object(map), anchor)
            }
            Event::SequenceStart(anchor, tag) => {
                self.check_tag(tag.as_ref(), pointer, mark)?;
                let mut values = Vec::new();
                loop {
                    let at = child(pointer, &values.len().to_string());
                    let (event, mark) = self.event(&at)?;
                    if event == Event::SequenceEnd {
                        break;
                    }
                    values.push(self.value(event, mark, &at, depth + 1)?);
                }
                (Value::Array(values), anchor)
            }
            Event::Alias(anchor) => {
                self.aliases += 1;
                if self.aliases > self.limits.max_aliases {
                    return Err(self.error(
                        "LIMIT_ALIASES",
                        "YAML alias limit exceeded",
                        pointer,
                        mark,
                    ));
                }
                let value = self.anchors.get(&anchor).ok_or_else(|| {
                    self.error(
                        "INVALID_ALIAS",
                        "Unknown or recursive YAML alias",
                        pointer,
                        mark,
                    )
                })?;
                let (nodes, extra_depth) = value_size(value);
                if self.nodes.saturating_add(nodes) > self.limits.max_nodes
                    || depth.saturating_add(extra_depth) > self.limits.max_depth
                {
                    return Err(self.error(
                        "LIMIT_NODES",
                        "Expanded alias exceeds document limits",
                        pointer,
                        mark,
                    ));
                }
                let bytes = crate::output::measure_json(
                    value,
                    self.limits
                        .max_total_bytes
                        .saturating_sub(self.copied_bytes),
                )
                .map_err(|_| {
                    self.error(
                        "LIMIT_TOTAL_BYTES",
                        "YAML alias copies exceed byte budget",
                        pointer,
                        mark,
                    )
                })?;
                self.copied_bytes += bytes;
                self.nodes += nodes;
                if let Some(includes) = self.anchor_includes.get(&anchor).cloned() {
                    for (suffix, target) in includes {
                        self.includes.insert(format!("{pointer}{suffix}"), target);
                    }
                }
                (value.clone(), 0)
            }
            _ => return Err(self.error("INVALID_YAML", "Expected a value", pointer, mark)),
        };
        if anchor != 0 {
            if self.raml {
                let includes = self
                    .includes
                    .iter()
                    .filter_map(|(at, target)| {
                        at.strip_prefix(pointer)
                            .filter(|suffix| suffix.is_empty() || suffix.starts_with('/'))
                            .map(|suffix| (suffix.to_owned(), target.clone()))
                    })
                    .collect();
                self.anchor_includes.insert(anchor, includes);
            }
            let bytes = crate::output::measure_json(
                &value,
                self.limits
                    .max_total_bytes
                    .saturating_sub(self.copied_bytes),
            )
            .map_err(|_| {
                self.error(
                    "LIMIT_TOTAL_BYTES",
                    "YAML anchor copies exceed byte budget",
                    pointer,
                    mark,
                )
            })?;
            self.copied_bytes += bytes;
            self.anchors.insert(anchor, value.clone());
        }
        Ok(value)
    }

    fn check_tag(&self, tag: Option<&Tag>, pointer: &str, mark: Marker) -> Result<()> {
        if let Some(tag) = tag {
            // Even inert custom tags can alter downstream meaning. Accept only
            // explicit string tags; the remaining scalar types use YAML 1.2 syntax.
            if tag.handle != "tag:yaml.org,2002:" || tag.suffix != "str" {
                return Err(self.error(
                    "UNSUPPORTED_YAML_TAG",
                    "Only the standard !!str YAML tag is accepted",
                    pointer,
                    mark,
                ));
            }
        }
        Ok(())
    }
}

pub(crate) fn value_size(value: &Value) -> (usize, usize) {
    let mut count = 0;
    let mut depth = 0;
    let mut queue = vec![(value, 0)];
    while let Some((value, current)) = queue.pop() {
        count += 1;
        depth = depth.max(current);
        match value {
            Value::Object(map) => queue.extend(map.values().map(|v| (v, current + 1))),
            Value::Array(values) => queue.extend(values.iter().map(|v| (v, current + 1))),
            _ => {}
        }
    }
    (count, depth)
}

fn scalar(text: &str) -> std::result::Result<Value, &'static str> {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => return Ok(Value::Null),
        "true" | "True" | "TRUE" => return Ok(Value::Bool(true)),
        "false" | "False" | "FALSE" => return Ok(Value::Bool(false)),
        _ => {}
    }
    if [".inf", "+.inf", "-.inf", ".nan"].contains(&text.to_ascii_lowercase().as_str()) {
        return Err("Non-finite numbers are not JSON values");
    }
    if let Ok(number) = text.parse::<i64>() {
        return Ok(Value::Number(number.into()));
    }
    if let Ok(number) = text.parse::<u64>() {
        return Ok(Value::Number(number.into()));
    }
    for (prefix, radix) in [("0x", 16), ("0o", 8)] {
        if let Some(digits) = text.strip_prefix(prefix)
            && let Ok(number) = u64::from_str_radix(digits, radix)
        {
            return Ok(Value::Number(number.into()));
        }
    }
    if let Some(number) = crate::numbers::parse_decimal(text) {
        return Ok(Value::Number(number));
    }
    Ok(Value::String(text.into()))
}

pub(crate) fn detect(value: &Value, uri: &str) -> Result<Dialect> {
    let at = Address {
        uri: uri.into(),
        pointer: String::new(),
    };
    if !value.is_object() {
        return Err(ParserError::new(
            "INVALID_DOCUMENT",
            "OpenAPI document must be an object",
            at,
        ));
    }
    if (value.get("swagger").is_some() || value.get("swaggerVersion").is_some())
        && value.get("openapi").is_some()
    {
        return Err(ParserError::new(
            "AMBIGUOUS_VERSION",
            "Document declares both swagger and openapi",
            at,
        ));
    }
    if value.get("swagger").and_then(Value::as_str) == Some("2.0") {
        return Ok(Dialect::Swagger2);
    }
    if value.get("swagger").is_some() || value.get("swaggerVersion").is_some() {
        return Err(ParserError::new(
            "UNSUPPORTED_SWAGGER_VERSION",
            "This parser supports Swagger 2.0; the declared Swagger version is unsupported",
            at,
        ));
    }
    if let Some(version) = value.get("openapi").and_then(Value::as_str) {
        let parts: Vec<_> = version.split('.').collect();
        if parts.len() == 3
            && parts[2]
                .split('-')
                .next()
                .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        {
            return match (parts[0], parts[1]) {
                ("3", "0") => Ok(Dialect::OpenApi30),
                ("3", "1") => Ok(Dialect::OpenApi31),
                ("3", "2") => Ok(Dialect::OpenApi32),
                _ => Err(ParserError::new(
                    "UNSUPPORTED_VERSION",
                    format!("Unsupported OpenAPI version {version}"),
                    at,
                )),
            };
        }
    }
    Err(ParserError::new(
        "UNSUPPORTED_VERSION",
        "Expected swagger: '2.0' or openapi: '3.0.x', '3.1.x', '3.2.x'",
        at,
    ))
}
