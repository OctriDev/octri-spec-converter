//! Effective operation inventory, retaining source provenance across references.

use crate::{
    Address, ApiOperation, Dialect, Inspection, Location, ReferenceEdge, SchemaEntry, Statistics,
};
use crate::{pointer, session::Session};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(crate) fn inspect(session: &Session<'_>) -> Inspection {
    let root = Address {
        uri: session.root_uri.clone(),
        pointer: String::new(),
    };
    let document = session.value(&root).unwrap_or(&Value::Null);
    let mut collector = Collector {
        session,
        document,
        operations: Vec::new(),
        active: BTreeSet::new(),
    };
    for (collection, kind) in [("paths", "path"), ("webhooks", "webhook")] {
        if let Some(paths) = document.get(collection).and_then(Value::as_object) {
            for path in paths.keys() {
                if collection == "paths" && !path.starts_with('/') {
                    continue;
                }
                collector.path(
                    path,
                    Address {
                        uri: root.uri.clone(),
                        pointer: pointer::child(&format!("/{collection}"), path),
                    },
                    kind,
                );
            }
        }
    }
    let schema_path = if session.dialect == Dialect::Swagger2 {
        "/definitions"
    } else {
        "/components/schemas"
    };
    let usage = ReferenceUsageIndex::new(&session.references);
    let schemas: Vec<_> = document
        .pointer(schema_path)
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, value)| {
            let address = Address {
                uri: root.uri.clone(),
                pointer: pointer::child(schema_path, name),
            };
            SchemaEntry {
                name: name.clone(),
                location: session.locate(&address),
                schema: value.clone(),
                used_by: usage.used_by(&address),
            }
        })
        .collect();
    let operations = collector.operations;
    Inspection {
        dialect: session.dialect,
        version: document
            .get("openapi")
            .or_else(|| document.get("swagger"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        info: document.get("info").cloned().unwrap_or_else(|| json!({})),
        security_schemes: {
            let path = if session.dialect == Dialect::Swagger2 {
                "/securityDefinitions"
            } else {
                "/components/securitySchemes"
            };
            Value::Object(
                document
                    .pointer(path)
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                    .map(|(name, _)| {
                        (
                            name.clone(),
                            session.resolved_api_value(&Address {
                                uri: root.uri.clone(),
                                pointer: pointer::child(path, name),
                            }),
                        )
                    })
                    .collect(),
            )
        },
        statistics: Statistics {
            operations: operations.len(),
            paths: document
                .get("paths")
                .and_then(Value::as_object)
                .map_or(0, |p| p.keys().filter(|k| k.starts_with('/')).count()),
            schemas: schemas.len(),
            documents: session.sources.len(),
            references: session.references.len(),
            unresolved: session.references.iter().filter(|e| !e.resolved).count(),
        },
        operations,
        schemas,
        references: session.references.clone(),
    }
}

struct Collector<'a, 'b> {
    session: &'a Session<'b>,
    document: &'a Value,
    operations: Vec<ApiOperation>,
    active: BTreeSet<Address>,
}

impl Collector<'_, '_> {
    fn resolved(&self, address: Address) -> Value {
        self.session.resolved_api_value(&address)
    }

    fn parameters(&self, address: &Address) -> Vec<Value> {
        self.session
            .value(address)
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .enumerate()
                    .map(|(i, _)| {
                        self.resolved(Address {
                            uri: address.uri.clone(),
                            pointer: pointer::child(&address.pointer, &i.to_string()),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn path(&mut self, path: &str, address: Address, kind: &str) {
        let fields = self.session.api_fields(&address, true);
        let address = self.session.follow(address);
        if self.active.len() > self.session.options.limits.max_depth
            || !self.active.insert(address.clone())
        {
            return;
        }
        let mut methods = vec![
            "get", "put", "post", "delete", "options", "head", "patch", "trace",
        ];
        if self.session.dialect == Dialect::OpenApi32 {
            methods.push("query");
        }
        let mut entries: Vec<_> = methods
            .into_iter()
            .filter(|method| {
                fields
                    .get(*method)
                    .and_then(|at| self.session.value(at))
                    .is_some_and(Value::is_object)
            })
            .filter_map(|m| {
                fields
                    .get(m)
                    .cloned()
                    .map(|address| (m.to_ascii_uppercase(), address))
            })
            .collect();
        if self.session.dialect == Dialect::OpenApi32
            && let Some(additional) = fields
                .get("additionalOperations")
                .and_then(|at| self.session.value(at))
                .and_then(Value::as_object)
            && let Some(base) = fields.get("additionalOperations")
        {
            entries.extend(additional.keys().map(|method| {
                (
                    method.clone(),
                    Address {
                        uri: base.uri.clone(),
                        pointer: pointer::child(&base.pointer, method),
                    },
                )
            }));
        }
        for (method, operation_address) in entries {
            let Some(operation) = self
                .session
                .value(&operation_address)
                .and_then(Value::as_object)
            else {
                continue;
            };
            let inherited = fields
                .get("parameters")
                .map(|address| self.parameters(address))
                .unwrap_or_default();
            let own = self.parameters(&Address {
                uri: operation_address.uri.clone(),
                pointer: pointer::child(&operation_address.pointer, "parameters"),
            });
            let mut parameters: Vec<Value> = Vec::new();
            for parameter in inherited.into_iter().chain(own) {
                let key = (parameter.get("in"), parameter.get("name"));
                if let Some(index) = parameters
                    .iter()
                    .position(|p| (p.get("in"), p.get("name")) == key)
                {
                    parameters[index] = parameter;
                } else {
                    parameters.push(parameter);
                }
            }
            let responses: BTreeMap<_, _> = operation
                .get("responses")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .map(|(status, _)| {
                    (
                        status.clone(),
                        self.resolved(Address {
                            uri: operation_address.uri.clone(),
                            pointer: pointer::child(
                                &pointer::child(&operation_address.pointer, "responses"),
                                status,
                            ),
                        }),
                    )
                })
                .collect();
            let swagger_servers = if self.session.dialect == Dialect::Swagger2 {
                let schemes = self
                    .document
                    .get("schemes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_else(|| vec![json!("")]);
                schemes.iter().map(|scheme| {
                    let scheme = scheme.as_str().unwrap_or("");
                    let host = self.document.get("host").and_then(Value::as_str).unwrap_or("");
                    let base = self.document.get("basePath").and_then(Value::as_str).unwrap_or("/");
                    json!({"url": format!("{}{}{base}", if scheme.is_empty() { String::new() } else { format!("{scheme}:") }, if host.is_empty() { String::new() } else { format!("//{host}") })})
                }).collect()
            } else {
                Vec::new()
            };
            self.operations.push(ApiOperation {
                method,
                path: path.into(),
                kind: kind.into(),
                operation_id: operation
                    .get("operationId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                summary: operation
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                description: operation
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                extensions: operation
                    .iter()
                    .filter(|(key, _)| key.starts_with("x-"))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
                tags: operation
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|t| {
                        t.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                deprecated: operation
                    .get("deprecated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                location: self.session.locate(&operation_address),
                parameters,
                security: array(
                    operation
                        .get("security")
                        .or_else(|| self.document.get("security")),
                ),
                servers: operation
                    .get("servers")
                    .or_else(|| fields.get("servers").and_then(|at| self.session.value(at)))
                    .or_else(|| self.document.get("servers"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or(swagger_servers),
                request_body: operation.get("requestBody").map(|_| {
                    self.resolved(Address {
                        uri: operation_address.uri.clone(),
                        pointer: pointer::child(&operation_address.pointer, "requestBody"),
                    })
                }),
                responses,
            });
            if let Some(callbacks) = operation.get("callbacks").and_then(Value::as_object) {
                for name in callbacks.keys() {
                    let callback = self.session.follow(Address {
                        uri: operation_address.uri.clone(),
                        pointer: pointer::child(
                            &pointer::child(&operation_address.pointer, "callbacks"),
                            name,
                        ),
                    });
                    if let Some(paths) = self.session.value(&callback).and_then(Value::as_object) {
                        for expression in paths.keys().filter(|p| !p.starts_with("x-")) {
                            self.path(
                                expression,
                                Address {
                                    uri: callback.uri.clone(),
                                    pointer: pointer::child(&callback.pointer, expression),
                                },
                                "callback",
                            );
                        }
                    }
                }
            }
        }
        self.active.remove(&address);
    }
}
fn array(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Reverse lookup of reference targets. Range queries find references into a
/// schema or any descendant without scanning every document edge per schema.
struct ReferenceUsageIndex<'a> {
    edges: &'a [ReferenceEdge],
    targets: BTreeMap<Address, Vec<usize>>,
}
impl<'a> ReferenceUsageIndex<'a> {
    fn new(edges: &'a [ReferenceEdge]) -> Self {
        let mut targets: BTreeMap<Address, Vec<usize>> = BTreeMap::new();
        for (index, edge) in edges.iter().enumerate() {
            if let Some(target) = &edge.target {
                targets
                    .entry(target.address.clone())
                    .or_default()
                    .push(index);
            }
        }
        Self { edges, targets }
    }
    fn used_by(&self, schema: &Address) -> Vec<Location> {
        let mut queue = VecDeque::from([schema.clone()]);
        let mut visited = BTreeSet::new();
        let mut used = BTreeMap::new();
        while let Some(address) = queue.pop_front() {
            if !visited.insert(address.clone()) {
                continue;
            }
            let start = Address {
                uri: address.uri.clone(),
                pointer: format!("{}/", address.pointer),
            };
            let descendants = self
                .targets
                .range(start.clone()..)
                .take_while(|(target, _)| {
                    target.uri == start.uri && target.pointer.starts_with(&start.pointer)
                })
                .map(|(_, edges)| edges);
            for indices in self.targets.get(&address).into_iter().chain(descendants) {
                for index in indices {
                    let from = &self.edges[*index].from;
                    if used.insert(from.address.clone(), from.clone()).is_none() {
                        let parent = Address {
                            uri: from.address.uri.clone(),
                            pointer: from
                                .address
                                .pointer
                                .rsplit_once('/')
                                .map_or("", |(parent, _)| parent)
                                .into(),
                        };
                        queue.push_back(parent);
                    }
                }
            }
        }
        used.into_values().collect()
    }
}
