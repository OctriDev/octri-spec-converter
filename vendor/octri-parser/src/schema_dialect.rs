//! JSON Schema dialects whose reference and schema-position rules we implement.

use serde_json::Value;

/// Default interpretation of a foreign schema. An authored `$schema` overrides
/// this hint; source bytes and physical addresses are never changed by the hint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SchemaDialect {
    Draft4,
    Draft7,
    Draft201909,
    #[default]
    Draft202012,
}

impl SchemaDialect {
    pub fn uri(self) -> &'static str {
        match self {
            Self::Draft4 => "http://json-schema.org/draft-04/schema#",
            Self::Draft7 => "http://json-schema.org/draft-07/schema#",
            Self::Draft201909 => "https://json-schema.org/draft/2019-09/schema",
            Self::Draft202012 => "https://json-schema.org/draft/2020-12/schema",
        }
    }

    pub fn ignores_reference_siblings(self) -> bool {
        matches!(self, Self::Draft4 | Self::Draft7)
    }

    pub(crate) fn identifier(self) -> &'static str {
        if self == Self::Draft4 { "id" } else { "$id" }
    }

    pub(crate) fn declared(value: &Value) -> Option<Self> {
        value
            .get("$schema")
            .and_then(Value::as_str)
            .and_then(Self::from_uri)
    }

    /// Recognize supported schema dialect identifiers. Unknown vocabularies
    /// retain the parser's existing custom-dialect diagnostic behavior.
    pub fn from_uri(uri: &str) -> Option<Self> {
        match uri.strip_suffix('#').unwrap_or(uri) {
            "http://json-schema.org/draft-04/schema"
            | "https://json-schema.org/draft-04/schema" => Some(Self::Draft4),
            "http://json-schema.org/draft-07/schema"
            | "https://json-schema.org/draft-07/schema" => Some(Self::Draft7),
            "http://json-schema.org/draft/2019-09/schema"
            | "https://json-schema.org/draft/2019-09/schema" => Some(Self::Draft201909),
            "https://json-schema.org/draft/2020-12/schema"
            | "https://spec.openapis.org/oas/3.1/dialect/base"
            | "https://spec.openapis.org/oas/3.2/dialect/base" => Some(Self::Draft202012),
            _ => None,
        }
    }
}

/// A schema subtree embedded in a foreign document, with its format's default
/// dialect. Registry roots make identifiers available without selecting payloads.
#[derive(Debug, Clone)]
pub struct SchemaRoot {
    pub address: crate::Address,
    pub dialect: SchemaDialect,
}

impl From<crate::Address> for SchemaRoot {
    fn from(address: crate::Address) -> Self {
        Self {
            address,
            dialect: SchemaDialect::default(),
        }
    }
}
