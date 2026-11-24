//! JSON transport helpers shared by the Node and WASM adapters.
//! No host capabilities or parser decisions are implemented here.

use crate::{Address, Limits, ParserError, Request, Result};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

pub const MAX_REQUEST_BYTES: usize = 128 * 1024 * 1024;
const MAX_TRANSPORT_DEPTH: usize = 256;
const ENCODE_FAILURE: &str =
    r#"{"Err":{"message":"Cannot encode parser result","diagnostics":[]}}"#;
pub(crate) const RESERVED_NUMBER_KEY: &str = "$serde_json::private::Number";

/// Reject serde_json's reserved arbitrary-number transport key before decoding;
/// without this, that literal object key can be mistaken for a number. Source
/// content inside a JSON string is checked when that source is parsed.
/// Returns the deepest container nesting so transport decoding can refuse
/// input that would overflow serde's recursion.
pub(crate) fn scan(uri: &str, text: &str) -> Result<usize> {
    let bytes = text.as_bytes();
    let mut offset = 0;
    let mut depth = 0usize;
    let mut deepest = 0;
    while offset < bytes.len() {
        match bytes[offset] {
            b'{' | b'[' => {
                depth += 1;
                deepest = deepest.max(depth);
                offset += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                offset += 1;
            }
            b'"' => {}
            _ => offset += 1,
        }
        if bytes.get(offset) != Some(&b'"') {
            continue;
        }
        let start = offset;
        offset += 1;
        while offset < bytes.len() {
            let byte = bytes[offset];
            offset += 1;
            if byte == b'"' {
                break;
            }
            if byte == b'\\' && offset < bytes.len() {
                offset += 1;
            }
        }
        let end = offset;
        while offset < bytes.len() && bytes[offset].is_ascii_whitespace() {
            offset += 1;
        }
        if bytes.get(offset) == Some(&b':')
            && end - start <= RESERVED_NUMBER_KEY.len() * 6 + 2
            && serde_json::from_str::<String>(&text[start..end])
                .is_ok_and(|key| key == RESERVED_NUMBER_KEY)
        {
            let mut error = ParserError::new(
                "UNSAFE_KEY",
                "Reserved JSON numeric transport key; rename this field before conversion",
                Address::root(uri),
            );
            let location = &mut error.diagnostics[0].location;
            location.offset = Some(start);
            location.line = Some(text[..start].matches('\n').count() + 1);
            location.column = Some(
                text[..start]
                    .rsplit('\n')
                    .next()
                    .unwrap_or("")
                    .chars()
                    .count()
                    + 1,
            );
            return Err(error);
        }
    }
    Ok(deepest)
}

/// Decode a bounded binding/report envelope with room for the source document's
/// own nesting plus its transport wrappers. Syntax validation remains serde's.
pub fn decode_json<T: DeserializeOwned>(uri: &str, text: &str) -> Result<T> {
    if text.len() > MAX_REQUEST_BYTES {
        return Err(ParserError::new(
            "LIMIT_REQUEST_BYTES",
            "JSON transport exceeds 128 MiB",
            Address::root(uri),
        ));
    }
    if scan(uri, text)? > MAX_TRANSPORT_DEPTH {
        return Err(ParserError::new(
            "LIMIT_DEPTH",
            "JSON transport exceeds 256 nesting levels",
            Address::root(uri),
        ));
    }
    let mut decoder = serde_json::Deserializer::from_str(text);
    // scan() bounded the depth, which serde's own 128-level limit would cut short.
    decoder.disable_recursion_limit();
    let value = T::deserialize(&mut decoder).and_then(|value| decoder.end().map(|()| value));
    value.map_err(|error| ParserError::new("INVALID_JSON", error.to_string(), Address::root(uri)))
}

pub fn request(text: &str) -> Result<Request> {
    decode_json("", text).map_err(|mut error| {
        let diagnostic = &mut error.diagnostics[0];
        if diagnostic.code == "INVALID_JSON" {
            diagnostic.code = "INVALID_REQUEST".into();
        }
        error
    })
}

pub fn serialize_document(text: &str, format: &str, limits: &str) -> String {
    encode(
        serde_json::from_str(limits)
            .map_err(|error| {
                ParserError::new("INVALID_OPTIONS", error.to_string(), Address::default())
            })
            .and_then(|limits| serialize_with(text, format, &limits)),
    )
}

pub fn serialize_with(text: &str, format: &str, limits: &Limits) -> Result<String> {
    let document: Value = decode_json("", text)?;
    match format {
        "json" => crate::serialize(&document, limits),
        "yaml" => crate::serialize_yaml(&document, limits),
        _ => Err(ParserError::new(
            "INVALID_OUTPUT_FORMAT",
            "Expected json or yaml output format",
            Address::default(),
        )),
    }
}

pub fn encode<T: Serialize>(result: Result<T>) -> String {
    serde_json::to_string(&result).unwrap_or_else(|_| ENCODE_FAILURE.into())
}

/// Encode binding-only location tables without repeating each source URI and
/// pointer. The JS facade restores the public report shape. Rust/CLI reports
/// continue to use their ordinary serde representation.
pub fn encode_report(result: Result<crate::Report>) -> String {
    match result {
        Ok(report) => encode(Ok(BindingReport::new(&report))),
        Err(error) => encode::<()>(Err(error)),
    }
}

/// Foreign JSON decoders often widen their entire temporary input string when
/// it contains a single non-BMP character. ASCII escapes bound that temporary
/// representation without changing any decoded string or numeric value.
pub fn encode_report_ascii(result: Result<crate::Report>) -> String {
    match result {
        Ok(report) => encode_ascii(Ok(BindingReport::new(&report))),
        Err(error) => encode_ascii::<()>(Err(error)),
    }
}

fn encode_ascii<T: Serialize>(result: Result<T>) -> String {
    let mut bytes = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, AsciiFormatter);
    if result.serialize(&mut serializer).is_err() {
        return ENCODE_FAILURE.into();
    }
    // serde_json writes valid UTF-8; this formatter replaces non-ASCII scalars
    // only with standard ASCII JSON escapes. Keep the checked conversion here.
    String::from_utf8(bytes).unwrap_or_else(|_| ENCODE_FAILURE.into())
}

struct AsciiFormatter;
impl serde_json::ser::Formatter for AsciiFormatter {
    fn write_string_fragment<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> std::io::Result<()> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        if fragment.is_ascii() {
            return writer.write_all(fragment.as_bytes());
        }
        let mut start = 0;
        for (index, character) in fragment.char_indices() {
            if character.is_ascii() {
                continue;
            }
            writer.write_all(&fragment.as_bytes()[start..index])?;
            for unit in character.encode_utf16(&mut [0; 2]) {
                let value = *unit as usize;
                writer.write_all(&[
                    b'\\',
                    b'u',
                    HEX[value >> 12],
                    HEX[(value >> 8) & 15],
                    HEX[(value >> 4) & 15],
                    HEX[value & 15],
                ])?;
            }
            start = index + character.len_utf8();
        }
        writer.write_all(&fragment.as_bytes()[start..])
    }
}

/// Static JS document methods do not expose parser state. Send only their return
/// value or identity graph; execution and all limits are identical to full reports.
pub fn encode_document(result: Result<crate::DocumentOutput>) -> String {
    #[derive(Serialize)]
    struct DocumentResult<'a> {
        document: Option<&'a Value>,
        graph: Option<IdentityGraph<'a>>,
    }
    #[derive(Serialize)]
    struct IdentityGraph<'a> {
        root: crate::NodeId,
        nodes: &'a [crate::GraphNode],
    }
    match result {
        Ok(report) => encode(Ok(DocumentResult {
            document: report
                .graph
                .is_none()
                .then_some(report.document.as_ref())
                .flatten(),
            graph: report.graph.as_ref().map(|graph| IdentityGraph {
                root: graph.root,
                nodes: &graph.nodes,
            }),
        })),
        Err(error) => encode::<()>(Err(error)),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BindingReport<'a> {
    wire_version: u8,
    valid: bool,
    dialect: &'a Option<crate::Dialect>,
    document: &'a Option<Value>,
    diagnostics: &'a [crate::Diagnostic],
    sources: Vec<BindingSource<'a>>,
    references: &'a [crate::ReferenceEdge],
    registry: &'a crate::ReferenceIndex,
    #[serde(skip_serializing_if = "Option::is_none")]
    inspection: &'a Option<crate::Inspection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    graph: &'a Option<crate::Graph>,
}
impl<'a> BindingReport<'a> {
    fn new(report: &'a crate::Report) -> Self {
        Self {
            wire_version: 1,
            valid: report.valid,
            dialect: &report.dialect,
            document: &report.document,
            diagnostics: &report.diagnostics,
            sources: report.sources.iter().map(BindingSource).collect(),
            references: &report.references,
            registry: &report.registry,
            inspection: &report.inspection,
            graph: &report.graph,
        }
    }
}

struct BindingSource<'a>(&'a crate::SourceDocument);
impl Serialize for BindingSource<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let source = self.0;
        let mut fields = serializer.serialize_struct("BindingSource", 6)?;
        fields.serialize_field("uri", &source.uri)?;
        fields.serialize_field("text", &source.text)?;
        fields.serialize_field("document", &source.document)?;
        fields.serialize_field("locations", &BindingLocations(source))?;
        fields.serialize_field("bytes", &source.bytes)?;
        fields.serialize_field("generated", &source.generated)?;
        fields.end()
    }
}
struct BindingLocations<'a>(&'a crate::SourceDocument);
impl Serialize for BindingLocations<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut entries = serializer.serialize_map(Some(self.0.locations.len()))?;
        for (pointer, location) in &self.0.locations {
            if location.address.uri != self.0.uri || location.address.pointer != *pointer {
                // Preserve unusual caller-constructed reports without losing information.
                entries.serialize_entry(pointer, location)?;
            } else {
                let position = [location.line, location.column, location.offset];
                entries.serialize_entry(
                    pointer,
                    &position.iter().any(Option::is_some).then_some(position),
                )?;
            }
        }
        entries.end()
    }
}
