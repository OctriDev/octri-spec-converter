//! Reachability for schemas declared in a foreign protocol's component registry.
//! Indexing a resource identifier does not make that resource an output root.

use crate::Address;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct SchemaSelection {
    available: BTreeMap<String, BTreeSet<String>>,
    active: BTreeMap<String, BTreeSet<String>>,
}

/// Match complete JSON Pointer ancestors, never a sibling with a shared prefix.
/// The enclosing root preserves relative IDs, anchors and dynamic scope around
/// a selected fragment. Lookup cost depends on pointer depth, not registry size.
fn enclosing_root<'a>(roots: &BTreeSet<String>, mut pointer: &'a str) -> Option<&'a str> {
    let mut enclosing = None;
    loop {
        if roots.contains(pointer) {
            enclosing = Some(pointer);
        }
        let Some((parent, _)) = pointer.rsplit_once('/') else {
            return enclosing;
        };
        pointer = parent;
    }
}

impl SchemaSelection {
    pub fn new(available: &[Address], selected: &[Address]) -> Self {
        let mut registry = Self {
            available: BTreeMap::new(),
            active: BTreeMap::new(),
        };
        for root in available {
            registry
                .available
                .entry(root.uri.clone())
                .or_default()
                .insert(root.pointer.clone());
        }
        for root in selected {
            registry.activate(root);
        }
        registry
    }

    pub fn activate(&mut self, target: &Address) {
        let pointer = self
            .available
            .get(&target.uri)
            .and_then(|roots| enclosing_root(roots, &target.pointer))
            .unwrap_or(&target.pointer);
        self.active
            .entry(target.uri.clone())
            .or_default()
            .insert(pointer.to_owned());
    }

    pub fn includes(&self, address: &Address) -> bool {
        // A newly retrieved standalone schema is already a selected dependency.
        // Only explicitly supplied foreign registries contain inactive roots.
        !self.available.contains_key(&address.uri)
            || self
                .active
                .get(&address.uri)
                .and_then(|roots| enclosing_root(roots, &address.pointer))
                .is_some()
    }
}
