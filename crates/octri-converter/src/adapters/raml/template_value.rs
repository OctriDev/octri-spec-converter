//! Template values carry physical origins through substitution and merging.
use crate::{
    ConversionError, Limits, Result,
    source::{Sources, child},
};
use octri_parser::Address;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, hash_map::RandomState};
use std::hash::{BuildHasher, Hash, Hasher};

#[derive(Clone)]
pub(super) struct LocatedValue {
    pub origin: Address,
    pub kind: Kind,
}

#[derive(Clone)]
pub(super) enum Kind {
    Object(BTreeMap<String, LocatedValue>),
    Array(Vec<LocatedValue>),
    Scalar(Value),
}

impl LocatedValue {
    pub fn read(value: &Value, pointer: &str, sources: &Sources) -> Self {
        let origin = sources
            .origins
            .get(pointer)
            .cloned()
            .unwrap_or_else(|| Address {
                uri: sources.root.clone(),
                pointer: pointer.into(),
            });
        let kind = match value {
            Value::Object(fields) => Kind::Object(
                fields
                    .iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            Self::read(value, &child(pointer, key), sources),
                        )
                    })
                    .collect(),
            ),
            Value::Array(values) => Kind::Array(
                values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        Self::read(value, &child(pointer, &index.to_string()), sources)
                    })
                    .collect(),
            ),
            scalar => Kind::Scalar(scalar.clone()),
        };
        Self { origin, kind }
    }

    pub fn scalar(value: Value, origin: &Address) -> Self {
        Self {
            origin: origin.clone(),
            kind: Kind::Scalar(value),
        }
    }

    pub fn object(origin: &Address) -> Self {
        Self {
            origin: origin.clone(),
            kind: Kind::Object(BTreeMap::new()),
        }
    }

    pub fn get(&self, key: &str) -> Option<&Self> {
        match &self.kind {
            Kind::Object(fields) => fields.get(key),
            _ => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match &self.kind {
            Kind::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, node)| (key.clone(), node.to_value()))
                    .collect(),
            ),
            Kind::Array(values) => Value::Array(values.iter().map(Self::to_value).collect()),
            Kind::Scalar(value) => value.clone(),
        }
    }

    pub fn install_origins(&self, pointer: &str, sources: &mut Sources) {
        sources.origins.insert(pointer.into(), self.origin.clone());
        match &self.kind {
            Kind::Object(fields) => {
                for (key, node) in fields {
                    node.install_origins(&child(pointer, key), sources);
                }
            }
            Kind::Array(values) => {
                for (index, node) in values.iter().enumerate() {
                    node.install_origins(&child(pointer, &index.to_string()), sources);
                }
            }
            Kind::Scalar(_) => {}
        }
    }

    fn same_value(&self, other: &Self, budget: &mut MergeBudget<'_>, depth: usize) -> Result<bool> {
        budget.tick(depth)?;
        Ok(match (&self.kind, &other.kind) {
            (Kind::Object(left), Kind::Object(right)) => {
                if left.len() != right.len() {
                    return Ok(false);
                }
                for (key, value) in left {
                    let Some(other) = right.get(key) else {
                        return Ok(false);
                    };
                    if !value.same_value(other, budget, depth + 1)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Kind::Array(left), Kind::Array(right)) => {
                if left.len() != right.len() {
                    return Ok(false);
                }
                for (left, right) in left.iter().zip(right) {
                    if !left.same_value(right, budget, depth + 1)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Kind::Scalar(left), Kind::Scalar(right)) => left == right,
            _ => false,
        })
    }

    fn hash_value(
        &self,
        hasher: &mut impl Hasher,
        budget: &mut MergeBudget<'_>,
        depth: usize,
    ) -> Result<()> {
        budget.tick(depth)?;
        match &self.kind {
            Kind::Object(fields) => {
                0u8.hash(hasher);
                fields.len().hash(hasher);
                for (key, value) in fields {
                    key.hash(hasher);
                    value.hash_value(hasher, budget, depth + 1)?;
                }
            }
            Kind::Array(values) => {
                1u8.hash(hasher);
                values.len().hash(hasher);
                for value in values {
                    value.hash_value(hasher, budget, depth + 1)?;
                }
            }
            Kind::Scalar(value) => {
                2u8.hash(hasher);
                value.hash(hasher);
            }
        }
        Ok(())
    }
}

struct MergeBudget<'a> {
    limits: &'a Limits,
    visited: usize,
}

impl MergeBudget<'_> {
    fn tick(&mut self, depth: usize) -> Result<()> {
        self.visited = self.visited.saturating_add(1);
        if depth > self.limits.max_depth || self.visited > self.limits.max_nodes {
            return Err(ConversionError::new(
                "RAML_MERGE_LIMIT",
                "Template merge work exceeds configured node or depth limit",
            ));
        }
        Ok(())
    }
}

pub(super) fn merge(
    base: LocatedValue,
    overlay: LocatedValue,
    limits: &Limits,
    depth: usize,
) -> Result<LocatedValue> {
    merge_at(
        base,
        overlay,
        &mut MergeBudget { limits, visited: 0 },
        depth,
    )
}

fn merge_at(
    base: LocatedValue,
    overlay: LocatedValue,
    budget: &mut MergeBudget<'_>,
    depth: usize,
) -> Result<LocatedValue> {
    budget.tick(depth)?;
    let origin = overlay.origin;
    let kind = match (base.kind, overlay.kind) {
        (Kind::Object(mut base), Kind::Object(overlay)) => {
            for (key, value) in overlay {
                let value = if let Some(previous) = base.remove(&key) {
                    merge_at(previous, value, budget, depth + 1)?
                } else {
                    value
                };
                base.insert(key, value);
            }
            Kind::Object(base)
        }
        (Kind::Array(mut base), Kind::Array(overlay)) => {
            // Index by JSON value, not origin. Randomized buckets resist crafted
            // collisions; exact comparison keeps collisions from losing data.
            // Store indices so growing the output cannot invalidate references.
            let state = RandomState::new();
            let mut index: HashMap<u64, Vec<usize>> = HashMap::new();
            for (position, value) in base.iter().enumerate() {
                let mut hasher = state.build_hasher();
                value.hash_value(&mut hasher, budget, depth + 1)?;
                index.entry(hasher.finish()).or_default().push(position);
            }
            for value in overlay {
                let mut hasher = state.build_hasher();
                value.hash_value(&mut hasher, budget, depth + 1)?;
                let positions = index.entry(hasher.finish()).or_default();
                let mut duplicate = false;
                for position in positions.iter() {
                    if base[*position].same_value(&value, budget, depth + 1)? {
                        duplicate = true;
                        break;
                    }
                }
                if !duplicate {
                    if base.len() >= budget.limits.max_nodes {
                        return Err(ConversionError::new(
                            "RAML_MERGE_NODES",
                            "Merged collection exceeds node limit",
                        ));
                    }
                    positions.push(base.len());
                    base.push(value);
                }
            }
            if base.len() > budget.limits.max_nodes {
                return Err(ConversionError::new(
                    "RAML_MERGE_NODES",
                    "Merged collection exceeds node limit",
                ));
            }
            Kind::Array(base)
        }
        (_, replacement) => replacement,
    };
    Ok(LocatedValue { origin, kind })
}

/// Keep inherited optional methods until a concrete resource is known. Only
/// method keys at this resource level participate; schema and example fields
/// named `get?` remain literal data.
pub(super) fn optional_methods(
    mut resource: LocatedValue,
    limits: &Limits,
) -> Result<LocatedValue> {
    if let Kind::Object(fields) = &mut resource.kind {
        let optional = fields
            .keys()
            .filter_map(|key| {
                key.strip_suffix('?')
                    .filter(|name| super::super::common::method(name))
                    .map(|name| (key.clone(), name.to_owned()))
            })
            .collect::<Vec<_>>();
        for (key, name) in optional {
            if let Some(optional) = fields.remove(&key)
                && let Some(concrete) = fields.remove(&name)
            {
                fields.insert(name, merge(optional, concrete, limits, 0)?);
            }
        }
    }
    Ok(resource)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, ConversionRequest};
    use octri_parser::Input;
    use serde_json::json;

    fn located(value: &Value, uri: &str) -> Result<LocatedValue> {
        let sources = Sources::new(&ConversionRequest {
            input: Input {
                uri: uri.into(),
                content: Some(value.to_string()),
                generated: false,
            },
            files: Default::default(),
            config: Config::default(),
        })?;
        Ok(LocatedValue::read(value, "", &sources))
    }

    #[test]
    fn array_merge_preserves_exact_values_order_and_first_origins()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let first: Value = serde_json::from_str(r#"[{"value":9007199254740993.25}]"#)?;
        let second: Value = serde_json::from_str(
            r#"[{"value":9007199254740993.25},{"value":9007199254740993.5}]"#,
        )?;
        let merged = merge(
            located(&first, "memory://base.json")?,
            located(&second, "memory://overlay.json")?,
            &Limits::default(),
            0,
        )?;
        assert_eq!(merged.to_value(), second);
        let Kind::Array(values) = merged.kind else {
            return Err("array expected".into());
        };
        assert_eq!(values[0].origin.uri, "memory://base.json");
        assert_eq!(values[1].origin.uri, "memory://overlay.json");
        Ok(())
    }

    #[test]
    fn small_output_cannot_bypass_merge_work_budget()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let left = located(&json!([1, 1]), "memory://base.json")?;
        let right = located(&json!([1, 1]), "memory://overlay.json")?;
        let limits = Limits {
            max_nodes: 5,
            ..Limits::default()
        };
        let error = merge(left, right, &limits, 0)
            .err()
            .ok_or("merge budget bypassed")?;
        assert_eq!(error.code, "RAML_MERGE_LIMIT");
        Ok(())
    }
}
