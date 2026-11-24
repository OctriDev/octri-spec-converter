//! JSON Pointer and URI fragment handling. Decode fragments exactly once.

use crate::{Address, ParserError, Result};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use url::Url;

pub fn child(pointer: &str, key: &str) -> String {
    let mut output = String::with_capacity(pointer.len() + key.len() + 1);
    output.push_str(pointer);
    output.push('/');
    if !key.contains(['~', '/']) {
        output.push_str(key);
    } else {
        for character in key.chars() {
            match character {
                '~' => output.push_str("~0"),
                '/' => output.push_str("~1"),
                character => output.push(character),
            }
        }
    }
    output
}

pub(crate) fn checked_url(reference: &str, base: &str, at: &Address) -> Result<Url> {
    let bytes = reference.as_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && (i + 2 >= bytes.len()
                || !bytes[i + 1].is_ascii_hexdigit()
                || !bytes[i + 2].is_ascii_hexdigit())
        {
            return Err(ParserError::new(
                "INVALID_REFERENCE_URI",
                "Reference contains an invalid percent escape",
                at.clone(),
            ));
        }
    }
    let base =
        Url::parse(base).map_err(|e| ParserError::new("INVALID_URI", e.to_string(), at.clone()))?;
    base.join(reference)
        .map_err(|e| ParserError::new("INVALID_REFERENCE_URI", e.to_string(), at.clone()))
}

pub(crate) fn fragment(url: &Url, at: &Address) -> Result<String> {
    let value = percent_decode_str(url.fragment().unwrap_or(""))
        .decode_utf8()
        .map_err(|_| {
            ParserError::new("INVALID_REFERENCE_URI", "Fragment is not UTF-8", at.clone())
        })?
        .into_owned();
    if value.starts_with('/') {
        let bytes = value.as_bytes();
        for (i, byte) in bytes.iter().enumerate() {
            if *byte == b'~' && !matches!(bytes.get(i + 1), Some(b'0' | b'1')) {
                return Err(ParserError::new(
                    "INVALID_POINTER",
                    "JSON Pointer contains an invalid tilde escape",
                    at.clone(),
                ));
            }
        }
    }
    Ok(value)
}

pub(crate) fn document_uri(url: &Url) -> String {
    let mut url = url.clone();
    url.set_fragment(None);
    url.to_string()
}

/// Characters with URI meaning are escaped; pointer separators and escapes stay.
const FRAGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'/')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

pub(crate) fn reference_to(pointer: &str) -> String {
    format!("#{}", utf8_percent_encode(pointer, FRAGMENT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn child_pointers_round_trip_escaped_keys_and_references_escape_uri_characters() {
        let keys = [
            "a/b",
            "a~b",
            "a~1b",
            "",
            "a#%?",
            "a😀b",
            "01",
            "-",
            "__proto__",
        ];
        for first in keys {
            for second in keys {
                let root = json!({ first: { second: true } });
                let path = child(&child("", first), second);
                assert_eq!(root.pointer(&path), Some(&json!(true)), "{path}");
            }
        }
        assert_eq!(reference_to("/a 😀#%?"), "#/a%20%F0%9F%98%80%23%25%3F");
    }
}
