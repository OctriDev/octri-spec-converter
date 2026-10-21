//! An arena makes shared and circular references explicit without unsafe code.
//! Node IDs remain stable for the lifetime of a graph. Adapters can reconstruct
//! object identity in JavaScript without reimplementing reference semantics.

use crate::{Address, CircularPolicy, Limits, ParserError, Result};
use crate::{
    pointer,
    session::Session,
    syntax::{self, Context},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub type NodeId = usize;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum GraphNode {
    Object(BTreeMap<String, NodeId>),
    Array(Vec<NodeId>),
    Scalar(Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Graph {
    pub root: NodeId,
    pub nodes: Vec<GraphNode>,
    pub origins: Vec<Address>,
    pub circular: bool,
}

impl Graph {
    /// Export a JSON tree. Cycles and excessive shared-subtree expansion fail
    /// explicitly instead of being silently truncated or exhausting memory.
    pub fn to_value(&self, limits: &Limits) -> Result<Value> {
        let mut active = BTreeSet::new();
        limits.check()?;
        let mut budget = ExportBudget::default();
        self.export::<true>(self.root, 0, limits, &mut active, &mut budget)
    }

    /// Check exactly the same expansion budgets without allocating a JSON tree.
    /// Document bindings return the identity graph and do not need that copy.
    pub fn check_output(&self, limits: &Limits) -> Result<()> {
        limits.check()?;
        self.export::<false>(
            self.root,
            0,
            limits,
            &mut BTreeSet::new(),
            &mut ExportBudget::default(),
        )
        .map(|_| ())
    }

    // Both entry points share traversal and budget logic. The const flag removes
    // tree allocation in check_output; its internal Null placeholder never escapes.
    fn export<const BUILD: bool>(
        &self,
        id: NodeId,
        depth: usize,
        limits: &Limits,
        active: &mut BTreeSet<NodeId>,
        budget: &mut ExportBudget,
    ) -> Result<Value> {
        let default = Address::default();
        let at = self.origins.get(id).unwrap_or(&default);
        budget.nodes += 1;
        if budget.nodes > limits.max_nodes || depth > limits.max_depth {
            return Err(ParserError::new(
                "LIMIT_OUTPUT_NODES",
                "Expanded graph exceeds output limits",
                at.clone(),
            ));
        }
        let container = matches!(
            self.nodes.get(id),
            Some(GraphNode::Object(_) | GraphNode::Array(_))
        );
        if container && !active.insert(id) {
            return Err(ParserError::new(
                "CIRCULAR_SERIALIZATION",
                "Circular graph cannot be serialized; use circular: 'ignore'",
                at.clone(),
            ));
        }
        let value = match self.nodes.get(id) {
            Some(GraphNode::Object(fields)) => {
                budget.charge(
                    2 + fields.len().saturating_sub(1) + fields.len(),
                    limits,
                    at,
                )?;
                let mut output = Map::new();
                for (key, child) in fields {
                    budget.json(key, limits, at)?;
                    let value = self.export::<BUILD>(*child, depth + 1, limits, active, budget)?;
                    if BUILD {
                        output.insert(key.clone(), value);
                    }
                }
                if BUILD {
                    Value::Object(output)
                } else {
                    Value::Null
                }
            }
            Some(GraphNode::Array(values)) => {
                budget.charge(2 + values.len().saturating_sub(1), limits, at)?;
                let mut output = Vec::new();
                for child in values {
                    let value = self.export::<BUILD>(*child, depth + 1, limits, active, budget)?;
                    if BUILD {
                        output.push(value);
                    }
                }
                if BUILD {
                    Value::Array(output)
                } else {
                    Value::Null
                }
            }
            Some(GraphNode::Scalar(value)) => {
                budget.json(value, limits, at)?;
                if BUILD { value.clone() } else { Value::Null }
            }
            None => {
                return Err(ParserError::new(
                    "INVALID_GRAPH",
                    "Graph contains an unknown node ID",
                    at.clone(),
                ));
            }
        };
        if container {
            active.remove(&id);
        }
        Ok(value)
    }
}

pub(crate) fn dereference(session: &Session<'_>) -> Result<Graph> {
    let mut builder = Builder {
        session,
        graph: Graph {
            root: 0,
            nodes: Vec::new(),
            origins: Vec::new(),
            circular: false,
        },
        memo: HashMap::new(),
        active: HashSet::new(),
        targets: session
            .references
            .iter()
            .filter_map(|edge| edge.target.as_ref().map(|target| target.address.clone()))
            .collect(),
        copied_bytes: 0,
    };
    let address = Address {
        uri: session.root_uri.clone(),
        pointer: String::new(),
    };
    builder.graph.root = builder.build(&address, Context::Api, 0)?;
    Ok(builder.graph)
}

struct Builder<'a, 'b> {
    session: &'a Session<'b>,
    graph: Graph,
    memo: HashMap<Address, NodeId>,
    active: HashSet<Address>,
    targets: HashSet<Address>,
    copied_bytes: usize,
}

impl Builder<'_, '_> {
    fn allocate(&mut self, address: &Address) -> Result<NodeId> {
        if self.graph.nodes.len() >= self.session.options.limits.max_nodes {
            return Err(ParserError::new(
                "LIMIT_OUTPUT_NODES",
                "Dereferenced graph exceeds node limit",
                address.clone(),
            ));
        }
        let id = self.graph.nodes.len();
        self.graph.nodes.push(GraphNode::Scalar(Value::Null));
        self.graph.origins.push(address.clone());
        Ok(id)
    }

    fn build(&mut self, address: &Address, context: Context, depth: usize) -> Result<NodeId> {
        let value = self.session.value(address).ok_or_else(|| {
            ParserError::new(
                "UNRESOLVED_REFERENCE",
                "Missing reference target",
                address.clone(),
            )
        })?;
        self.build_value(address, value, context, depth)
    }

    fn build_value(
        &mut self,
        address: &Address,
        value: &Value,
        context: Context,
        depth: usize,
    ) -> Result<NodeId> {
        if depth > self.session.options.limits.max_depth {
            return Err(ParserError::new(
                "LIMIT_DEPTH",
                "Reference expansion exceeds nesting limit",
                address.clone(),
            ));
        }
        let active = self.active.contains(address);
        if let Some(id) = self.memo.get(address)
            && (!self.session.options.dereference.clone_references || active)
        {
            return Ok(*id);
        }
        let target = if matches!(context, Context::Api | Context::Schema) {
            self.session
                .edge(address, "$ref")
                .and_then(|edge| edge.target.as_ref())
                .map(|l| l.address.clone())
        } else {
            None
        };
        if let Some(target) = target {
            if let Some(canonical) = self.reference_only_cycle(address)? {
                self.graph.circular = true;
                return match self.session.options.dereference.circular {
                    CircularPolicy::Error => Err(ParserError::new(
                        "CIRCULAR_REFERENCE",
                        "Reference-only cycle detected",
                        address.clone(),
                    )),
                    CircularPolicy::Ignore => {
                        self.copy_object(address, value, Context::Literal, depth)
                    }
                    CircularPolicy::Preserve => self.canonical_cycle(address, &canonical),
                };
            }
            if self.active.contains(&target) {
                self.graph.circular = true;
                match self.session.options.dereference.circular {
                    CircularPolicy::Error => {
                        return Err(ParserError::new(
                            "CIRCULAR_REFERENCE",
                            "Circular reference detected",
                            address.clone(),
                        ));
                    }
                    CircularPolicy::Ignore => {
                        return self.copy_object(address, value, Context::Literal, depth);
                    }
                    CircularPolicy::Preserve => {
                        if let Some(id) = self.memo.get(&target) {
                            return Ok(*id);
                        }
                    }
                }
            }
            // Reference-only cycles (A -> B -> A) have no concrete object to
            // materialize. Preserve the reference edge rather than inventing data.
            if self.active.contains(address) {
                return self.copy_object(address, value, Context::Literal, depth);
            }
            let siblings = value
                .as_object()
                .map(|map| {
                    map.iter()
                        .filter(|(k, _)| k.as_str() != "$ref")
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let schema_conjunction = context == Context::Schema
                && self.session.dialect.modern_schema()
                && self
                    .session
                    .contexts
                    .get(address)
                    .is_none_or(|context| !context.schema_dialect.ignores_reference_siblings())
                && !siblings.is_empty();
            let reference_annotations = context != Context::Schema
                && self.session.dialect.modern_schema()
                && siblings
                    .iter()
                    .any(|(k, _)| matches!(k.as_str(), "summary" | "description"));
            let path_item = syntax::is_path_item(&address.pointer, value);
            let preserve_siblings = schema_conjunction || reference_annotations || path_item;
            self.active.insert(address.clone());
            if preserve_siblings {
                let id = self.allocate(address)?;
                self.memo.insert(address.clone(), id);
                let target_context = self
                    .session
                    .contexts
                    .get(&target)
                    .map_or(context, |c| c.context);
                let target_id = self.build(&target, target_context, depth + 1)?;
                let mut fields = BTreeMap::new();
                if schema_conjunction {
                    let all_of_id = self.allocate(address)?;
                    self.graph.nodes[all_of_id] = GraphNode::Array(vec![target_id]);
                    fields.insert("allOf".into(), all_of_id);
                } else if let GraphNode::Object(target_fields) = &self.graph.nodes[target_id] {
                    fields = target_fields.clone();
                }
                for (key, value) in siblings {
                    if reference_annotations
                        && !path_item
                        && !matches!(key.as_str(), "summary" | "description")
                    {
                        continue;
                    }
                    let child = Address {
                        uri: address.uri.clone(),
                        pointer: pointer::child(&address.pointer, key),
                    };
                    let child_context =
                        syntax::next_in_dialect(context, key, value, self.session.dialect);
                    let child_id = self.build_value(&child, value, child_context, depth + 1)?;
                    if schema_conjunction && key == "allOf" {
                        if let Some(all_of_id) = fields.get("allOf").copied() {
                            let mut items = vec![target_id];
                            if let GraphNode::Array(own) = &self.graph.nodes[child_id] {
                                items.extend(own);
                            }
                            self.graph.nodes[all_of_id] = GraphNode::Array(items);
                        }
                    } else {
                        fields.insert(key.clone(), child_id);
                    }
                }
                self.graph.nodes[id] = GraphNode::Object(fields);
                self.active.remove(address);
                return Ok(id);
            }
            let target_context = self
                .session
                .contexts
                .get(&target)
                .map_or(context, |c| c.context);
            let id = self.build(&target, target_context, depth + 1)?;
            self.memo.insert(address.clone(), id);
            self.active.remove(address);
            return Ok(id);
        }
        self.copy_object(address, value, context, depth)
    }

    fn reference_only_cycle(&self, start: &Address) -> Result<Option<Address>> {
        let mut chain = Vec::new();
        let mut address = start.clone();
        loop {
            if let Some(index) = chain.iter().position(|seen| seen == &address) {
                return Ok(chain[index..].iter().min().cloned());
            }
            if chain.len() > self.session.options.limits.max_depth {
                return Err(ParserError::new(
                    "LIMIT_DEPTH",
                    "Reference chain exceeds nesting limit",
                    start.clone(),
                ));
            }
            let Some(value) = self.session.value(&address).and_then(Value::as_object) else {
                return Ok(None);
            };
            if value.len() != 1 || !value.contains_key("$ref") {
                return Ok(None);
            }
            let Some(target) = self
                .session
                .edge(&address, "$ref")
                .and_then(|edge| edge.target.as_ref())
            else {
                return Ok(None);
            };
            chain.push(address);
            address = target.address.clone();
        }
    }

    fn canonical_cycle(&mut self, address: &Address, canonical: &Address) -> Result<NodeId> {
        if let Some(id) = self.memo.get(canonical).copied() {
            self.memo.insert(address.clone(), id);
            return Ok(id);
        }
        let context = self.session.contexts.get(address);
        let reference = match context {
            Some(context)
                if context.resource.uri == canonical.uri
                    && canonical
                        .pointer
                        .strip_prefix(&context.resource.pointer)
                        .is_some_and(|tail| tail.is_empty() || tail.starts_with('/')) =>
            {
                pointer::reference_to(&canonical.pointer[context.resource.pointer.len()..])
            }
            _ => format!(
                "{}{}",
                canonical.uri,
                pointer::reference_to(&canonical.pointer)
            ),
        };
        let id = self.allocate(canonical)?;
        let reference_id = self.allocate(canonical)?;
        self.graph.nodes[reference_id] = GraphNode::Scalar(Value::String(reference));
        self.graph.nodes[id] = GraphNode::Object(BTreeMap::from([("$ref".into(), reference_id)]));
        self.memo.insert(canonical.clone(), id);
        self.memo.insert(address.clone(), id);
        Ok(id)
    }

    fn copy_object(
        &mut self,
        address: &Address,
        value: &Value,
        context: Context,
        depth: usize,
    ) -> Result<NodeId> {
        let bytes = match value {
            Value::String(value) => value.len(),
            Value::Object(map) => map.keys().map(String::len).sum(),
            _ => 0,
        };
        self.copied_bytes = self.copied_bytes.saturating_add(bytes);
        if self.copied_bytes > self.session.options.limits.max_output_bytes {
            return Err(ParserError::new(
                "LIMIT_OUTPUT_BYTES",
                "Graph copy exceeds byte budget",
                address.clone(),
            ));
        }
        let id = self.allocate(address)?;
        // Ordinary children are visited once. Only reference targets can be
        // re-entered, so they need memo entries and ancestor-cycle tracking.
        // Reference holders keep their explicit handling in build_value.
        let target = self.targets.contains(address);
        if target {
            self.memo.insert(address.clone(), id);
        }
        let container = target && (value.is_object() || value.is_array());
        if container {
            self.active.insert(address.clone());
        }
        self.graph.nodes[id] =
            match value {
                Value::Object(map) => {
                    let mut fields = BTreeMap::new();
                    for (key, child_value) in map {
                        let child = Address {
                            uri: address.uri.clone(),
                            pointer: pointer::child(&address.pointer, key),
                        };
                        fields.insert(
                            key.clone(),
                            self.build_value(
                                &child,
                                child_value,
                                syntax::next_schema(
                                    context,
                                    key,
                                    child_value,
                                    value.get("items").is_some_and(Value::is_array),
                                    self.session.dialect,
                                    self.session
                                        .contexts
                                        .get(address)
                                        .map(|node| node.schema_dialect)
                                        .unwrap_or_default(),
                                ),
                                depth + 1,
                            )?,
                        );
                    }
                    if context == Context::Schema
                        && self.session.dialect.modern_schema()
                        && self.session.contexts.get(address).is_some_and(|context| {
                            context.schema_dialect.ignores_reference_siblings()
                        })
                        && !fields.contains_key("$schema")
                    {
                        let dialect = self.allocate(address)?;
                        self.graph.nodes[dialect] = GraphNode::Scalar(Value::String(
                            self.session
                                .contexts
                                .get(address)
                                .map(|context| context.schema_dialect)
                                .unwrap_or_default()
                                .uri()
                                .into(),
                        ));
                        fields.insert("$schema".into(), dialect);
                    }
                    GraphNode::Object(fields)
                }
                Value::Array(values) => {
                    let mut items = Vec::new();
                    for (i, child_value) in values.iter().enumerate() {
                        let child = Address {
                            uri: address.uri.clone(),
                            pointer: pointer::child(&address.pointer, &i.to_string()),
                        };
                        items.push(self.build_value(
                            &child,
                            child_value,
                            if context == Context::SchemaArray {
                                Context::Schema
                            } else {
                                context
                            },
                            depth + 1,
                        )?);
                    }
                    GraphNode::Array(items)
                }
                _ => GraphNode::Scalar(value.clone()),
            };
        if container {
            self.active.remove(address);
        }
        Ok(id)
    }
}

#[derive(Default)]
struct ExportBudget {
    nodes: usize,
    bytes: usize,
}
impl ExportBudget {
    fn charge(&mut self, bytes: usize, limits: &Limits, at: &Address) -> Result<()> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > limits.max_output_bytes {
            return Err(ParserError::new(
                "LIMIT_OUTPUT_BYTES",
                "Expanded graph exceeds byte budget",
                at.clone(),
            ));
        }
        Ok(())
    }
    fn json(&mut self, value: &impl serde::Serialize, limits: &Limits, at: &Address) -> Result<()> {
        let bytes =
            crate::output::measure_json(value, limits.max_output_bytes.saturating_sub(self.bytes))
                .map_err(|_| {
                    ParserError::new(
                        "LIMIT_OUTPUT_BYTES",
                        "Expanded graph exceeds byte budget",
                        at.clone(),
                    )
                })?;
        self.charge(bytes, limits, at)
    }
}
