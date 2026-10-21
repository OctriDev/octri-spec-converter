//! Resolve WSDL 2.0 interface ancestry once, with bounded work and no I/O.
use super::catalog::{Catalog, Kind, WSDL20};
use crate::adapters::xml::Name;
use crate::{ConversionError, Limits, Result};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn resolve(
    catalog: &Catalog,
    limits: &Limits,
) -> Result<BTreeMap<Name, BTreeSet<Name>>> {
    let mut graph = Graph {
        catalog,
        limits,
        active: BTreeSet::new(),
        resolved: BTreeMap::new(),
        work: 0,
    };
    for (kind, name) in catalog.declarations.keys() {
        if *kind == Kind::Interface {
            graph.visit(name)?;
        }
    }
    // A diamond reaches the same physical member once. Distinct declaring
    // interfaces have different required {parent} components, so their members
    // cannot be equivalent even when their XML payload declarations match
    // (WSDL 2.0 sections 2.3, 2.4 and 2.15).
    for resolved in graph.resolved.values() {
        for local in ["operation", "fault"] {
            let mut members = BTreeSet::new();
            for ancestor in &resolved.ancestors {
                let declaration = catalog.declaration(Kind::Interface, ancestor)?;
                for at in catalog.children(&declaration.node, WSDL20, local)? {
                    graph.work += 1;
                    check_budget(graph.work, limits)?;
                    let node = catalog.node(&at)?;
                    let name = node.attribute("name").ok_or_else(|| {
                        ConversionError::new(
                            "WSDL_INTERFACE_MEMBER",
                            "Interface member requires name",
                        )
                    })?;
                    if !members.insert(Name::new(&ancestor.namespace, name)) {
                        return Err(ConversionError::new(
                            "WSDL_INTERFACE_CONFLICT",
                            format!(
                                "Multiple {local} declarations share QName {{{}}}{name} but have distinct declaring interfaces; their required parent components are not equivalent.",
                                ancestor.namespace
                            ),
                        ));
                    }
                }
            }
        }
    }
    Ok(graph
        .resolved
        .into_iter()
        .map(|(name, resolved)| (name, resolved.ancestors))
        .collect())
}

#[derive(Clone)]
struct Resolved {
    ancestors: BTreeSet<Name>,
    depth: usize,
}

struct Graph<'a> {
    catalog: &'a Catalog,
    limits: &'a Limits,
    active: BTreeSet<Name>,
    resolved: BTreeMap<Name, Resolved>,
    work: usize,
}

impl Graph<'_> {
    fn visit(&mut self, name: &Name) -> Result<Resolved> {
        if let Some(resolved) = self.resolved.get(name) {
            return Ok(resolved.clone());
        }
        if !self.active.insert(name.clone()) {
            return Err(ConversionError::new(
                "WSDL_INTERFACE_CYCLE",
                "Interface inheritance contains a cycle",
            ));
        }
        if self.active.len() > self.limits.max_depth {
            return Err(ConversionError::new(
                "WSDL_INTERFACE_LIMIT",
                "Interface inheritance exceeds configured depth",
            ));
        }
        let declaration = self.catalog.declaration(Kind::Interface, name)?;
        let node = self.catalog.node(&declaration.node)?;
        let mut bases = BTreeSet::new();
        let mut resolved = Resolved {
            ancestors: BTreeSet::from([name.clone()]),
            depth: 1,
        };
        for base in node.attribute("extends").unwrap_or("").split_whitespace() {
            let base = node.qname(base)?;
            if !bases.insert(base.clone()) {
                return Err(ConversionError::new(
                    "WSDL_INTERFACE_DUPLICATE_BASE",
                    "Interface extends lists one QName more than once",
                ));
            }
            let inherited = self.visit(&base)?;
            self.work += inherited.ancestors.len();
            check_budget(self.work, self.limits)?;
            // Cached ancestors still contribute their full path length. Depth
            // admission must not depend on declaration or traversal order.
            resolved.depth = resolved.depth.max(inherited.depth + 1);
            if resolved.depth > self.limits.max_depth {
                return Err(ConversionError::new(
                    "WSDL_INTERFACE_LIMIT",
                    "Interface inheritance exceeds configured depth",
                ));
            }
            resolved.ancestors.extend(inherited.ancestors);
        }
        self.work += 1;
        check_budget(self.work, self.limits)?;
        self.active.remove(name);
        self.resolved.insert(name.clone(), resolved.clone());
        Ok(resolved)
    }
}

fn check_budget(work: usize, limits: &Limits) -> Result<()> {
    if work > limits.max_nodes {
        return Err(ConversionError::new(
            "WSDL_INTERFACE_LIMIT",
            "Interface inheritance exceeds configured node budget",
        ));
    }
    Ok(())
}
