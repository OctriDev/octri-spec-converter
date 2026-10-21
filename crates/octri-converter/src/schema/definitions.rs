//! Legacy Schema Objects have no reusable-definition keyword. Move definitions
//! into API components after strict validation and static reference resolution.
use crate::{
    Classification, ConversionError, Result, api,
    source::{Sources, child, hash},
    trace::Trace,
};
use octri_parser::{ReferenceEdge, SchemaDialect};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

struct Definition {
    name: String,
    dialect: SchemaDialect,
    remove: bool,
}

/// Empty reserved locations carry no assertions. A directly selected map still
/// acts as a reference target, so retain it for explicit legacy migration review.
pub(super) fn remove_unused_empty_maps(
    document: &mut Value,
    default: SchemaDialect,
    edges: &[ReferenceEdge],
    sources: &Sources,
    trace: &mut Trace,
) -> bool {
    let targets: BTreeSet<_> = edges
        .iter()
        .filter_map(|edge| edge.target.as_ref())
        .map(|target| target.address.pointer.as_str())
        .collect();
    let mut removed = false;
    api::schemas(document, &mut |schema, pointer| {
        super::walk_in_dialect(schema, pointer, default, &mut |schema, at, dialect| {
            if dialect.ignores_reference_siblings() && schema.get("$ref").is_some() {
                return;
            }
            let Some(map) = schema.as_object_mut() else {
                return;
            };
            for keyword in ["definitions", "$defs"] {
                if keyword == "$defs" && dialect.ignores_reference_siblings() {
                    continue;
                }
                let location = child(at, keyword);
                if map
                    .get(keyword)
                    .and_then(Value::as_object)
                    .is_some_and(|map| map.is_empty())
                    && !targets.contains(location.as_str())
                {
                    map.remove(keyword);
                    removed = true;
                    trace.record(sources, &sources.root, &location, &[at.into()],
                        "EMPTY_SCHEMA_DEFINITIONS_REMOVED", Classification::Transformed,
                        "Removed an empty reusable-schema map after source validation and reference resolution; no schema or reference target was discarded", None);
                }
            }
        });
    });
    removed
}

pub(super) struct Definitions(BTreeMap<String, Definition>);

impl Definitions {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn removes_container(&self, pointer: &str) -> bool {
        self.0.iter().any(|(path, definition)| {
            definition.remove
                && path
                    .rsplit_once('/')
                    .is_some_and(|(parent, _)| parent == pointer)
        })
    }

    pub fn plan(document: &mut Value, default: SchemaDialect, edges: &[ReferenceEdge]) -> Self {
        let mut candidates = BTreeMap::new();
        let mut barriers = Vec::new();
        api::schemas(document, &mut |schema, pointer| {
            super::walk_in_dialect(schema, pointer, default, &mut |schema, at, dialect| {
                if dialect.ignores_reference_siblings() && schema.get("$ref").is_some() {
                    barriers.push((at.to_owned(), dialect));
                    return;
                }
                for key in ["$defs", "definitions"] {
                    if key == "$defs" && dialect.ignores_reference_siblings() {
                        continue;
                    }
                    for name in schema
                        .get(key)
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|map| map.keys())
                    {
                        candidates.insert(child(&child(at, key), name), (dialect, true));
                    }
                }
            });
        });
        // An explicitly reached schema may live below ignored legacy siblings.
        // Extract that target before the owning Reference Object is collapsed.
        for target in edges.iter().filter_map(|edge| edge.target.as_ref()) {
            if let Some((_, dialect)) = barriers
                .iter()
                .filter(|(barrier, _)| descendant(barrier, &target.address.pointer))
                .max_by_key(|(barrier, _)| barrier.len())
            {
                candidates
                    .entry(target.address.pointer.clone())
                    .or_insert((*dialect, false));
            }
        }
        let mut names: BTreeSet<_> = document
            .pointer("/components/schemas")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|map| map.keys().cloned())
            .collect();
        Self(
            candidates
                .into_iter()
                .map(|(pointer, (dialect, remove))| {
                    let mut name = format!("Hoisted_{}", &hash(pointer.as_bytes())[..16]);
                    while !names.insert(name.clone()) {
                        name.push('_');
                    }
                    (
                        pointer,
                        Definition {
                            name,
                            dialect,
                            remove,
                        },
                    )
                })
                .collect(),
        )
    }

    pub fn target(&self, pointer: &str) -> String {
        let mut parent = pointer;
        loop {
            if let Some(definition) = self.0.get(parent) {
                return format!(
                    "{}{}",
                    child("/components/schemas", &definition.name),
                    &pointer[parent.len()..]
                );
            }
            let Some((prefix, _)) = parent.rsplit_once('/') else {
                return pointer.into();
            };
            parent = prefix;
        }
    }

    pub fn apply(self, document: &mut Value, sources: &Sources, trace: &mut Trace) -> Result<()> {
        let mut plans: Vec<_> = self.0.into_iter().collect();
        plans.sort_by(|left, right| right.0.len().cmp(&left.0.len()).then(left.0.cmp(&right.0)));
        for (pointer, definition) in plans {
            let mut schema = if definition.remove {
                let (container, key) = pointer.rsplit_once('/').ok_or_else(missing)?;
                let key = key.replace("~1", "/").replace("~0", "~");
                let map = document
                    .pointer_mut(container)
                    .and_then(Value::as_object_mut)
                    .ok_or_else(missing)?;
                let schema = map.remove(&key).ok_or_else(missing)?;
                if map.is_empty() {
                    let (owner, keyword) = container.rsplit_once('/').ok_or_else(missing)?;
                    document
                        .pointer_mut(owner)
                        .and_then(Value::as_object_mut)
                        .ok_or_else(missing)?
                        .remove(keyword);
                }
                schema
            } else {
                let value = document.pointer_mut(&pointer).ok_or_else(missing)?;
                std::mem::replace(
                    value,
                    json!({"$ref":format!("#{}",child("/components/schemas",&definition.name))}),
                )
            };
            if let Some(map) = schema.as_object_mut() {
                map.entry("$schema")
                    .or_insert_with(|| json!(definition.dialect.uri()));
            }
            let components = document
                .as_object_mut()
                .ok_or_else(missing)?
                .entry("components")
                .or_insert_with(|| json!({}));
            let schemas = components
                .as_object_mut()
                .ok_or_else(missing)?
                .entry("schemas")
                .or_insert_with(|| json!({}));
            if schemas
                .as_object_mut()
                .ok_or_else(missing)?
                .insert(definition.name.clone(), schema)
                .is_some()
            {
                return Err(ConversionError::new(
                    "SCHEMA_NAME_COLLISION",
                    "Hoisted schema conflicts with an existing component",
                ));
            }
            trace.record(sources,&sources.root,"",&[],"SCHEMA_DEFINITION_HOISTED",Classification::Transformed,
                format!("Reusable schema at pre-migration path {pointer} moved to component {}; static references retain their targets",definition.name),
                Some(&json!({"from":pointer,"component":definition.name})));
        }
        Ok(())
    }
}

fn descendant(parent: &str, pointer: &str) -> bool {
    pointer
        .strip_prefix(parent)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn missing() -> ConversionError {
    ConversionError::new(
        "SCHEMA_DEFINITION_MIGRATION",
        "Validated schema definition is missing during migration",
    )
}
