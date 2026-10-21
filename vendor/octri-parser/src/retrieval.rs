//! Retrieval redirects are host metadata, not JSON Schema identifiers or I/O grants.

use crate::{Address, ParserError, Result, pointer};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn normalize(
    supplied: &BTreeMap<String, String>,
    base: &str,
    limit: usize,
) -> Result<BTreeMap<String, String>> {
    let at = Address {
        uri: base.into(),
        pointer: String::new(),
    };
    if supplied.len() > limit {
        return Err(ParserError::new(
            "LIMIT_DOCUMENTS",
            "Retrieval alias budget exhausted",
            at,
        ));
    }
    let mut aliases = BTreeMap::new();
    for (requested, final_uri) in supplied {
        let requested = checked_uri(requested, base, &at)?;
        let final_uri = checked_uri(final_uri, base, &at)?;
        if aliases.insert(requested, final_uri).is_some() {
            return Err(ParserError::new(
                "DUPLICATE_RESOURCE",
                "Retrieval aliases normalize to the same URI",
                at,
            ));
        }
    }
    flatten(&aliases, &at)
}

pub(crate) fn checked_uri(uri: &str, base: &str, at: &Address) -> Result<String> {
    let url = pointer::checked_url(uri, base, at)?;
    if url.fragment().is_some() {
        return Err(ParserError::new(
            "INVALID_OPTIONS",
            "Retrieval URIs cannot contain fragments",
            at.clone(),
        ));
    }
    Ok(url.to_string())
}

/// Flatten once at registration to avoid chasing chains at every reference lookup.
pub(crate) fn flatten(
    aliases: &BTreeMap<String, String>,
    at: &Address,
) -> Result<BTreeMap<String, String>> {
    let mut flattened: BTreeMap<String, String> = BTreeMap::new();
    for requested in aliases.keys() {
        if flattened.contains_key(requested) {
            continue;
        }
        let mut seen = BTreeSet::new();
        let mut final_uri = requested;
        let mut path = Vec::new();
        while let Some(next) = aliases.get(final_uri) {
            if let Some(known) = flattened.get(final_uri) {
                final_uri = known;
                break;
            }
            if !seen.insert(final_uri) {
                return Err(ParserError::new(
                    "INVALID_OPTIONS",
                    "Retrieval aliases contain a cycle",
                    at.clone(),
                ));
            }
            path.push(final_uri.clone());
            final_uri = next;
        }
        let final_uri = final_uri.clone();
        for alias in path {
            flattened.insert(alias, final_uri.clone());
        }
    }
    Ok(flattened)
}

pub(crate) fn canonical<'a>(aliases: &'a BTreeMap<String, String>, uri: &'a str) -> &'a str {
    aliases.get(uri).map(String::as_str).unwrap_or(uri)
}
