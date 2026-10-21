//! Place exact wire methods in OpenAPI fixed fields or additionalOperations.

use crate::source::child;
use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub(super) enum HttpMethod<'a> {
    Fixed(&'static str),
    Additional(&'a str),
}

impl<'a> HttpMethod<'a> {
    pub fn parse(wire: &'a str) -> Option<Self> {
        if wire.is_empty()
            || !wire
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        {
            return None;
        }
        // HTTP method tokens are case-sensitive (RFC 9110 section 9.1).
        // Lowercase `get` is a custom method, distinct from fixed-field GET.
        let field = match wire {
            "GET" => "get",
            "PUT" => "put",
            "POST" => "post",
            "DELETE" => "delete",
            "OPTIONS" => "options",
            "HEAD" => "head",
            "PATCH" => "patch",
            "TRACE" => "trace",
            "QUERY" => "query",
            _ => return Some(Self::Additional(wire)),
        };
        Some(Self::Fixed(field))
    }

    pub fn pointer(self, path: &str) -> String {
        let path_item = child("/paths", path);
        match self {
            Self::Fixed(field) => child(&path_item, field),
            Self::Additional(wire) => child(&child(&path_item, "additionalOperations"), wire),
        }
    }

    pub fn insert(self, document: &mut Value, path: &str, operation: Value) {
        // The intermediate document must declare positions that its parser
        // traverses. The emitter later checks the caller's selected target;
        // older targets block rather than silently changing wire semantics.
        match self {
            Self::Fixed(field) => {
                if field == "query" {
                    document["openapi"] = json!("3.2.1");
                }
                document["paths"][path][field] = operation;
            }
            Self::Additional(wire) => {
                document["openapi"] = json!("3.2.1");
                document["paths"][path]["additionalOperations"][wire] = operation;
            }
        }
    }
}
