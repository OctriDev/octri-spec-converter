//! GraphQL input/output kinds and recursive type constraints precede projection.
use super::{Schema, TypeRef, builtin};
use crate::{ConversionError, Result};
use std::collections::{BTreeMap, BTreeSet};

fn invalid(message: impl Into<String>) -> ConversionError {
    ConversionError::new("GRAPHQL_SEMANTICS", message)
}

pub(super) fn validate(schema: &Schema) -> Result<()> {
    let mut roots = BTreeSet::new();
    if !schema.roots.iter().any(|(kind, _)| kind == "query") {
        return Err(invalid("GraphQL requires a query root"));
    }
    for (_, root) in &schema.roots {
        if !roots.insert(root) || schema.types.get(root).is_none_or(|ty| ty.kind != "OBJECT") {
            return Err(invalid(
                "Operation roots must be distinct declared object types",
            ));
        }
    }
    let mut input_edges = BTreeMap::new();
    let mut interface_edges = BTreeMap::new();
    for (name, ty) in &schema.types {
        if name.starts_with("__") {
            continue;
        }
        if !matches!(
            ty.kind.as_str(),
            "SCALAR" | "ENUM" | "OBJECT" | "INTERFACE" | "UNION" | "INPUT_OBJECT"
        ) {
            return Err(invalid(format!("Unknown kind {} for {name}", ty.kind)));
        }
        let mut fields = BTreeSet::new();
        for field in ty.fields.as_deref().unwrap_or_default() {
            if !fields.insert(&field.name) {
                return Err(invalid(format!("Duplicate field {name}.{}", field.name)));
            }
            reference(&field.ty, schema, false, 0)?;
            let mut arguments = BTreeSet::new();
            for argument in &field.args {
                if !arguments.insert(&argument.name) {
                    return Err(invalid(format!(
                        "Duplicate argument on {name}.{}",
                        field.name
                    )));
                }
                reference(&argument.ty, schema, true, 0)?;
            }
        }
        let mut input_fields = BTreeSet::new();
        let mut required_edges = Vec::new();
        for field in ty.input_fields.as_deref().unwrap_or_default() {
            if !input_fields.insert(&field.name) {
                return Err(invalid(format!(
                    "Duplicate input field {name}.{}",
                    field.name
                )));
            }
            reference(&field.ty, schema, true, 0)?;
            if field.ty.kind == "NON_NULL"
                && let Some(inner) = field.ty.of_type.as_deref()
                && inner.kind != "LIST"
                && let Some(target) = &inner.name
                && schema
                    .types
                    .get(target)
                    .is_some_and(|ty| ty.kind == "INPUT_OBJECT")
            {
                required_edges.push(target.clone());
            }
        }
        input_edges.insert(name.clone(), required_edges);
        if matches!(ty.kind.as_str(), "OBJECT" | "INTERFACE") && fields.is_empty() {
            return Err(invalid(format!(
                "{name} must declare at least one output field"
            )));
        }
        if ty.kind == "INPUT_OBJECT" && input_fields.is_empty() {
            return Err(invalid(format!(
                "{name} must declare at least one input field"
            )));
        }
        let mut interfaces = Vec::new();
        for interface in ty.interfaces.as_deref().unwrap_or_default() {
            let target = interface
                .name
                .as_deref()
                .ok_or_else(|| invalid("Interface reference needs a name"))?;
            if schema
                .types
                .get(target)
                .is_none_or(|ty| ty.kind != "INTERFACE")
                || interfaces.iter().any(|name| name == target)
            {
                return Err(invalid(format!(
                    "{name} implements an unknown, duplicate or non-interface type"
                )));
            }
            interfaces.push(target.to_owned());
        }
        interface_edges.insert(name.clone(), interfaces);
        let mut members = BTreeSet::new();
        for member in ty.possible_types.as_deref().unwrap_or_default() {
            let target = member
                .name
                .as_deref()
                .ok_or_else(|| invalid("Abstract member needs a name"))?;
            if !members.insert(target)
                || schema
                    .types
                    .get(target)
                    .is_none_or(|ty| ty.kind != "OBJECT")
            {
                return Err(invalid(format!(
                    "{name} has duplicate or non-object concrete members"
                )));
            }
        }
        if ty.kind == "UNION" && members.is_empty() {
            return Err(invalid(format!("Union {name} has no concrete members")));
        }
    }
    acyclic(
        &input_edges,
        "Non-null input-object fields form an uninhabitable cycle",
    )?;
    acyclic(&interface_edges, "Interface inheritance is cyclic")
}

fn reference(reference: &TypeRef, schema: &Schema, input: bool, depth: usize) -> Result<()> {
    if depth > 128 {
        return Err(invalid("Type reference nesting exceeds 128"));
    }
    if matches!(reference.kind.as_str(), "NON_NULL" | "LIST") {
        let inner = reference
            .of_type
            .as_deref()
            .ok_or_else(|| invalid("Wrapped type needs ofType"))?;
        if reference.name.is_some() || (reference.kind == "NON_NULL" && inner.kind == "NON_NULL") {
            return Err(invalid("Malformed wrapped type reference"));
        }
        return self::reference(inner, schema, input, depth + 1);
    }
    let name = reference
        .name
        .as_deref()
        .ok_or_else(|| invalid("Named type needs a name"))?;
    let kind = if builtin(name) {
        "SCALAR"
    } else {
        schema
            .types
            .get(name)
            .map(|ty| ty.kind.as_str())
            .ok_or_else(|| invalid(format!("Unknown type {name}")))?
    };
    if reference.of_type.is_some() || (reference.kind != "NAMED" && reference.kind != kind) {
        return Err(invalid(format!(
            "Reference kind disagrees with declaration of {name}"
        )));
    }
    if (input && !matches!(kind, "SCALAR" | "ENUM" | "INPUT_OBJECT"))
        || (!input && kind == "INPUT_OBJECT")
    {
        return Err(invalid(format!(
            "{name} cannot be used in an {} position",
            if input { "input" } else { "output" }
        )));
    }
    Ok(())
}

fn acyclic(edges: &BTreeMap<String, Vec<String>>, message: &str) -> Result<()> {
    let mut pending = edges
        .iter()
        .map(|(name, targets)| (name.as_str(), targets.len()))
        .collect::<BTreeMap<_, _>>();
    let mut incoming = BTreeMap::<&str, Vec<&str>>::new();
    for (name, targets) in edges {
        for target in targets {
            incoming.entry(target).or_default().push(name);
        }
    }
    let mut ready = pending
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(name, _)| *name)
        .collect::<Vec<_>>();
    let mut removed = 0;
    while let Some(name) = ready.pop() {
        removed += 1;
        for parent in incoming.get(name).into_iter().flatten() {
            if let Some(count) = pending.get_mut(parent) {
                *count -= 1;
                if *count == 0 {
                    ready.push(parent);
                }
            }
        }
    }
    if removed != edges.len() {
        return Err(invalid(message));
    }
    Ok(())
}
