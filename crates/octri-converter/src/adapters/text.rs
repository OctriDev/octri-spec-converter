//! Preflight recursive text grammars before handing them to protocol parsers.
use crate::{ConversionError, Limits, Result};

pub(crate) fn check_nesting(text: &str, limits: &Limits) -> Result<()> {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut depth = 0usize;
    let mut tokens = 0usize;
    while index < bytes.len() {
        let current = bytes[index];
        if current == b'#' || bytes.get(index..index + 2) == Some(b"//") {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if bytes.get(index..index + 2) == Some(b"/*") {
            index += 2;
            while index + 1 < bytes.len() && bytes.get(index..index + 2) != Some(b"*/") {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            continue;
        }
        if matches!(current, b'"' | b'\'') {
            let triple = bytes.get(index..index + 3) == Some(b"\"\"\"");
            let width = if triple { 3 } else { 1 };
            index += width;
            while index < bytes.len() {
                if bytes[index] == b'\\' {
                    index = (index + 2).min(bytes.len());
                    continue;
                }
                if triple && bytes.get(index..index + 3) == Some(b"\"\"\"") {
                    index += 3;
                    break;
                }
                if !triple && bytes[index] == current {
                    index += 1;
                    break;
                }
                index += 1;
            }
            tokens += 1;
            continue;
        }
        if matches!(current, b'{' | b'[' | b'(') {
            depth += 1;
            tokens += 1;
            if depth > limits.max_depth {
                return Err(ConversionError::new(
                    "LIMIT_DEPTH",
                    "Protocol syntax nesting exceeds configured maximum",
                ));
            }
        } else if matches!(current, b'}' | b']' | b')') {
            depth = depth.saturating_sub(1);
        } else if !current.is_ascii_whitespace() {
            tokens += 1;
            while index + 1 < bytes.len()
                && (bytes[index + 1].is_ascii_alphanumeric() || bytes[index + 1] == b'_')
            {
                index += 1;
            }
        }
        if tokens > limits.max_nodes {
            return Err(ConversionError::new(
                "LIMIT_NODES",
                "Protocol syntax exceeds token budget",
            ));
        }
        index += 1;
    }
    Ok(())
}
