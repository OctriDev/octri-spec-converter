//! JSON Pointer and URI fragment handling. Decode fragments exactly once.

use crate::{Address, ParserError, Result};
use percent_encoding::percent_decode_str;
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

/// Borrow ordinary tokens instead of allocating two replacement strings for
/// every level of every source lookup. Escaped tokens follow serde_json's rules.
pub(crate) fn get<'a>(
    mut value: &'a serde_json::Value,
    pointer: &str,
) -> Option<&'a serde_json::Value> {
    use serde_json::Value;
    use std::borrow::Cow;
    if pointer.is_empty() {
        return Some(value);
    }
    for token in pointer.strip_prefix('/')?.split('/') {
        let token = if token.contains('~') {
            Cow::Owned(token.replace("~1", "/").replace("~0", "~"))
        } else {
            Cow::Borrowed(token)
        };
        value = match value {
            Value::Object(map) => map.get(token.as_ref())?,
            Value::Array(values) => {
                if token.starts_with('+') || token.starts_with('0') && token.len() != 1 {
                    return None;
                }
                values.get(token.parse::<usize>().ok()?)?
            }
            _ => return None,
        };
    }
    Some(value)
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

pub(crate) fn reference_to(pointer: &str) -> String {
    // Preserve pointer separators and escapes; encode characters with URI meaning.
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::with_capacity(pointer.len() + 1);
    output.push('#');
    for byte in pointer.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            output.push(char::from(byte));
        } else {
            output.push('%');
            output.push(char::from(HEX[(byte >> 4) as usize]));
            output.push(char::from(HEX[(byte & 15) as usize]));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn source_lookup_matches_serde_json_for_escaped_tokens_and_array_indexes() {
        let mut root =
            json!({"": {"items": ["first", {"a/b~c": 42}]}, "numeric": {"01":true,"+1":false}});
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
                root[first] = json!({second: [0, 1, 2]});
                let path = child(&child("", first), second);
                assert!(get(&root, &path).is_some(), "{path}");
                for suffix in [
                    "",
                    "/0",
                    "/1",
                    "/01",
                    "/+1",
                    "/-1",
                    "/-",
                    "/999999999999999999999999999",
                ] {
                    let pointer = format!("{path}{suffix}");
                    assert_eq!(get(&root, &pointer), root.pointer(&pointer), "{pointer}");
                }
            }
        }
        for pointer in ["", "missing", "/missing", "/~2", "/~01", "/numeric/01"] {
            assert_eq!(get(&root, pointer), root.pointer(pointer), "{pointer}");
        }
        assert_eq!(reference_to("/a 😀#%?"), "#/a%20%F0%9F%98%80%23%25%3F");
        assert_eq!(get(&Value::Null, ""), Some(&Value::Null));
    }
}
