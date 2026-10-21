//! JSON strings use UTF-16 escape pairs, which YAML's escape grammar does not.
//! Decode with the bounded JSON visitor, then locate tokens in the original text.
use crate::{Limits, Location, Result, SourceDocument};
use std::collections::BTreeMap;

pub(super) fn parse(
    uri: &str,
    text: String,
    limits: &Limits,
    locations: bool,
) -> Result<SourceDocument> {
    let mut source = super::generated::parse_text(uri, text, limits, locations)?;
    if !locations {
        return Ok(source);
    }
    let has_bom = source.text.starts_with('\u{feff}');
    let mut cursor = Cursor {
        text: &source.text,
        offset: if has_bom { '\u{feff}'.len_utf8() } else { 0 },
        line: 1,
        column: 1 + usize::from(has_bom),
        locations: &mut source.locations,
    };
    cursor.value("")?;
    source.generated = false;
    Ok(source)
}

/// This cursor receives already validated, depth-bounded JSON. It only records
/// token starts; serde_json remains responsible for syntax and string decoding.
struct Cursor<'a> {
    text: &'a str,
    offset: usize,
    line: usize,
    column: usize,
    locations: &'a mut BTreeMap<String, Location>,
}

impl Cursor<'_> {
    fn byte(&self) -> Option<u8> {
        self.text.as_bytes().get(self.offset).copied()
    }

    fn advance(&mut self) {
        let Some(byte) = self.byte() else { return };
        self.offset += 1;
        if byte == b'\n' {
            self.line += 1;
            self.column = 1;
        } else if byte & 0xc0 != 0x80 {
            // A UTF-8 continuation byte does not start a new source column.
            self.column += 1;
        }
    }

    fn whitespace(&mut self) {
        while self.byte().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.advance();
        }
    }

    fn string(&mut self) -> &str {
        let start = self.offset;
        self.advance();
        while let Some(byte) = self.byte() {
            self.advance();
            match byte {
                b'"' => break,
                b'\\' => self.advance(),
                _ => {}
            }
        }
        &self.text[start..self.offset]
    }

    fn value(&mut self, pointer: &str) -> Result<()> {
        self.whitespace();
        if let Some(location) = self.locations.get_mut(pointer) {
            location.line = Some(self.line);
            location.column = Some(self.column);
            location.offset = Some(self.offset);
        }
        match self.byte() {
            Some(b'{') => {
                self.advance();
                self.whitespace();
                while self.byte().is_some_and(|byte| byte != b'}') {
                    let key: String = serde_json::from_str(self.string()).map_err(|error| {
                        crate::ParserError::new(
                            "INVALID_JSON",
                            error.to_string(),
                            crate::Address::default(),
                        )
                    })?;
                    self.whitespace();
                    self.advance(); // colon
                    self.value(&crate::pointer::child(pointer, &key))?;
                    self.whitespace();
                    if self.byte() != Some(b',') {
                        break;
                    }
                    self.advance();
                    self.whitespace();
                }
                self.advance(); // closing brace
            }
            Some(b'[') => {
                self.advance();
                self.whitespace();
                let mut index = 0;
                while self.byte().is_some_and(|byte| byte != b']') {
                    self.value(&crate::pointer::child(pointer, &index.to_string()))?;
                    index += 1;
                    self.whitespace();
                    if self.byte() != Some(b',') {
                        break;
                    }
                    self.advance();
                    self.whitespace();
                }
                self.advance(); // closing bracket
            }
            Some(b'"') => {
                self.string();
            }
            _ => {
                while self.byte().is_some_and(|byte| {
                    !byte.is_ascii_whitespace() && !matches!(byte, b',' | b'}' | b']')
                }) {
                    self.advance();
                }
            }
        }
        Ok(())
    }
}
