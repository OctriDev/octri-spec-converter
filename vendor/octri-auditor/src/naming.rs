//! Conservative static naming profile. No generator execution is implied.
//!
//! Case and separator folding catches authored identifier collisions. Namespace
//! shortening is explicitly profiled; callers still execute their actual generator.

use octri_parser::ApiOperation;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

mod verbs;

/// Naming evidence is scoped to a namespace and HTTP method. A lone operation
/// or a split vote cannot establish a schema convention.
#[derive(Debug, Default)]
pub struct NamingConventions {
    namespaces: BTreeMap<(String, String), BTreeMap<String, usize>>,
    methods: BTreeMap<String, BTreeMap<String, usize>>,
    reserved: BTreeSet<String>,
}

impl NamingConventions {
    pub fn new(operations: &[ApiOperation]) -> Self {
        let mut conventions = Self::default();
        for operation in operations {
            let Some(id) = &operation.operation_id else {
                continue;
            };
            conventions.reserved.insert(camel(id).to_ascii_lowercase());
            let words = operation_words(id);
            let Some(first) = words.first().filter(|word| verbs::family(word).is_some()) else {
                continue;
            };
            if stacked_action(&words, operation) {
                continue;
            }
            *conventions
                .namespaces
                .entry((
                    namespace(operation, &Value::Null).join("."),
                    operation.method.clone(),
                ))
                .or_default()
                .entry(first.clone())
                .or_default() += 1;
            *conventions
                .methods
                .entry(operation.method.clone())
                .or_default()
                .entry(first.clone())
                .or_default() += 1;
        }
        conventions
    }

    fn preferred<'a>(&'a self, operation: &ApiOperation, action: &str) -> Option<&'a str> {
        let key = (
            namespace(operation, &Value::Null).join("."),
            operation.method.clone(),
        );
        let counts = self
            .namespaces
            .get(&key)
            .and_then(dominant_action)
            .or_else(|| {
                self.methods
                    .get(&operation.method)
                    .and_then(dominant_action)
            })?;
        // An action such as cancel/download must never turn into create/get
        // merely because unrelated operations use those prefixes.
        (verbs::family(counts) == verbs::family(action)
            && verbs::family(action) != Some(verbs::Family::Action))
        .then_some(counts)
    }
}

fn dominant_action(counts: &BTreeMap<String, usize>) -> Option<&str> {
    let total: usize = counts.values().sum();
    let (word, count) = counts.iter().max_by_key(|(_, count)| *count)?;
    (*count >= 2 && *count * 3 >= total * 2).then_some(word.as_str())
}

fn operation_words(id: &str) -> Vec<String> {
    let local = id.trim().rsplit(['/', '.']).next().unwrap_or("");
    let normalized = camel(local);
    let mut words = Vec::new();
    let mut start = 0;
    for (index, ch) in normalized.char_indices() {
        if ch.is_ascii_uppercase() || ch == '_' {
            if start < index {
                words.push(normalized[start..index].to_ascii_lowercase());
            }
            start = index + usize::from(ch == '_');
        }
    }
    if start < normalized.len() {
        words.push(normalized[start..].to_ascii_lowercase());
    }
    words
}

fn stacked_action(words: &[String], operation: &ApiOperation) -> bool {
    if words.len() < 3 || verbs::family(&words[0]).is_none() || verbs::family(&words[1]).is_none() {
        return false;
    }
    if words[0] != words[1] && !(words[0] == "get" && verbs::verbal_read(&words[1])) {
        return false;
    }
    // In getUpdate/update namespaces, `update` can be the resource noun.
    !namespace(operation, &Value::Null)
        .iter()
        .flat_map(|segment| operation_words(segment))
        .any(|resource| singular(&words[1]) == singular(&resource))
}

pub fn camel(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut cursor = 0;
    let mut words = Vec::new();
    while cursor < bytes.len() {
        if !bytes[cursor].is_ascii_alphanumeric() {
            cursor += 1;
            continue;
        }
        let start = cursor;
        if bytes[cursor].is_ascii_uppercase() {
            while cursor < bytes.len() && bytes[cursor].is_ascii_uppercase() {
                cursor += 1;
            }
            let capitals = cursor - start;
            if capitals >= 2
                && bytes.get(cursor) == Some(&b's')
                && bytes
                    .get(cursor + 1)
                    .is_none_or(|byte| !byte.is_ascii_lowercase())
            {
                cursor += 1;
            } else if bytes.get(cursor).is_some_and(u8::is_ascii_lowercase) {
                if capitals > 1 {
                    cursor -= 1;
                } else {
                    while cursor < bytes.len() && bytes[cursor].is_ascii_lowercase() {
                        cursor += 1;
                    }
                    consume_number_words(bytes, &mut cursor);
                }
            } else {
                let digits = cursor;
                while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                    cursor += 1;
                }
                if bytes.get(cursor).is_some_and(u8::is_ascii_lowercase) {
                    cursor = digits;
                }
            }
        } else if bytes[cursor].is_ascii_lowercase() {
            while cursor < bytes.len() && bytes[cursor].is_ascii_lowercase() {
                cursor += 1;
            }
            consume_number_words(bytes, &mut cursor);
        } else {
            while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                cursor += 1;
            }
            while cursor < bytes.len() && bytes[cursor].is_ascii_lowercase() {
                cursor += 1;
            }
        }
        words.push(&value[start..cursor]);
    }
    let mut output = String::new();
    for (index, word) in words.into_iter().enumerate() {
        let lower = word.to_ascii_lowercase();
        if index > 0 {
            output.push(lower.as_bytes()[0].to_ascii_uppercase() as char);
            output.push_str(&lower[1..]);
        } else {
            output.push_str(&lower);
        }
    }
    if value.ends_with('_') {
        output.push('_');
    }
    output
}

fn consume_number_words(bytes: &[u8], cursor: &mut usize) {
    while bytes.get(*cursor).is_some_and(u8::is_ascii_digit) {
        while bytes.get(*cursor).is_some_and(u8::is_ascii_digit) {
            *cursor += 1;
        }
        while bytes.get(*cursor).is_some_and(u8::is_ascii_lowercase) {
            *cursor += 1;
        }
    }
}

pub fn suggested_rename(
    operation: &ApiOperation,
    conventions: &mut NamingConventions,
) -> Option<String> {
    let current = operation.operation_id.as_deref()?.trim();
    let local = current.rsplit(['/', '.']).next()?;
    let words = operation_words(local);
    if words.is_empty() {
        return None;
    }
    let lower = &words;
    let leads = verbs::family(&lower[0]).is_some();
    // The action vocabulary is not an exhaustive English dictionary. An
    // unfamiliar first word may be an authored action, including domain verbs.
    // Casing, tags and HTTP methods cannot prove that it is a resource noun.
    // Preserve uncertain authored names rather than inventing a second action.
    if !leads {
        return None;
    }
    let trailing = lower
        .last()
        .filter(|word| verbs::family(word).is_some())
        .map(|_| words.len() - 1);
    let stem = |word: &str| word.strip_suffix('s').unwrap_or(word).to_owned();
    let namespace = operation
        .tags
        .first()
        .map(String::as_str)
        .unwrap_or_else(|| {
            operation
                .path
                .split('/')
                .rfind(|word| !word.is_empty() && !word.starts_with('{'))
                .unwrap_or("")
        });
    let namespace = namespace
        .split(['_', '-', '/', '.', ' '])
        .rfind(|word| !word.is_empty())
        .unwrap_or("")
        .to_lowercase();
    let repeated = lower
        .windows(2)
        .any(|pair| stem(&pair[0]) == stem(&pair[1]));
    let repeated_namespace = !namespace.is_empty()
        && lower
            .iter()
            .filter(|word| stem(word) == stem(&namespace))
            .count()
            > 1;
    let stacked = stacked_action(&words, operation);
    if leads && !stacked && (!repeated && !repeated_namespace || trailing.is_none()) {
        return None;
    }
    let action_index = if stacked { 1 } else { 0 };
    let mut subject: Vec<&str> = Vec::new();
    for (index, word) in words.iter().enumerate() {
        if index == action_index
            || leads && index == 0
            || index > 0 && stem(&lower[index]) == stem(&lower[index - 1])
        {
            continue;
        }
        if repeated_namespace
            && stem(&lower[index]) == stem(&namespace)
            && subject
                .iter()
                .any(|kept| stem(&kept.to_lowercase()) == stem(&lower[index]))
        {
            continue;
        }
        subject.push(word.as_str());
    }
    if subject.is_empty() {
        return None;
    }
    let verb = lower[action_index].as_str();
    let verb = conventions.preferred(operation, verb).unwrap_or(verb);
    let proposal = camel(&format!("{verb} {}", subject.join(" ")));
    // A stylistic repair must never introduce a duplicate authored identifier.
    // Keep the original when no collision-free name can be inferred.
    if proposal.is_empty() || proposal == current || proposal == local {
        return None;
    }
    // Reserve emitted proposals too: selecting several repairs in one preview
    // must not create an identifier collision absent from the authored source.
    conventions
        .reserved
        .insert(proposal.to_ascii_lowercase())
        .then_some(proposal)
}

fn method_verb(method: &str) -> &str {
    match method {
        "GET" => "get",
        "POST" => "create",
        "PUT" | "PATCH" => "update",
        "DELETE" => "delete",
        "HEAD" => "head",
        "OPTIONS" => "options",
        "TRACE" => "trace",
        _ => "call",
    }
}

pub fn identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_lowercase())
        && chars.all(|ch| ch.is_ascii_alphanumeric())
}

pub fn namespace(operation: &ApiOperation, authored: &Value) -> Vec<String> {
    let configured = authored.get("x-octri-sdk-tags").and_then(|value| {
        value.as_str().or_else(|| {
            value
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .find(|tag| !tag.trim().is_empty())
        })
    });
    let explicit = configured.or_else(|| {
        authored
            .get("x-interfacer-namespace")
            .and_then(Value::as_str)
    });
    if let Some(explicit) = explicit {
        let segments: Vec<_> = explicit
            .split(['/', '.', '\\'])
            .map(camel)
            .filter(|s| !s.is_empty())
            .collect();
        if !segments.is_empty() {
            return segments;
        }
    }
    if let Some(tag) = operation
        .tags
        .first()
        .map(|tag| camel(tag))
        .filter(|s| !s.is_empty())
    {
        return vec![tag];
    }
    vec![
        operation
            .path
            .split('/')
            .filter(|segment| !segment.starts_with('{') && !segment.is_empty())
            .map(camel)
            .find(|segment| {
                !segment.starts_with('v') || !segment[1..].chars().all(|ch| ch.is_ascii_digit())
            })
            .unwrap_or_else(|| "misc".into()),
    ]
}

pub fn public_name(operation: &ApiOperation, authored: &Value) -> String {
    let namespace = namespace(operation, authored);
    let mut name = camel(operation.operation_id.as_deref().unwrap_or(""));
    if let Some(last) = namespace.last() {
        let singular = singular(last);
        let plural = if last.ends_with('s') {
            last.clone()
        } else if let Some(stem) = last.strip_suffix('y') {
            format!("{stem}ies")
        } else {
            format!("{last}s")
        };
        for token in [last.as_str(), singular.as_str(), plural.as_str()] {
            if token.is_empty() {
                continue;
            }
            let mut title = token.to_string();
            if let Some(first) = title.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            let candidate = name
                .find(&title)
                .or_else(|| name.starts_with(token).then_some(0));
            if let Some(index) = candidate {
                let end = index + title.len();
                if end == name.len()
                    || name.as_bytes()[end].is_ascii_uppercase()
                    || name.as_bytes()[end].is_ascii_digit()
                {
                    name.replace_range(index..end, "");
                    break;
                }
            }
        }
    }
    if name.is_empty() {
        name = camel(operation.operation_id.as_deref().unwrap_or("operation"));
    }
    format!("{}.{}", namespace.join("."), camel(&name)).to_ascii_lowercase()
}

pub fn suggested(operation: &ApiOperation) -> String {
    let mut value = method_verb(&operation.method).to_owned();
    for segment in operation
        .path
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        let word = if segment.starts_with('{') {
            format!("by {}", segment.trim_matches(['{', '}']))
        } else {
            segment.into()
        };
        let word = camel(&word);
        if let Some(first) = word.chars().next() {
            value.push(first.to_ascii_uppercase());
            value.push_str(&word[1..]);
        }
    }
    value
}

fn singular(value: &str) -> String {
    if let Some(stem) = value.strip_suffix("ies") {
        return format!("{stem}y");
    }
    if value == "statuses" {
        return "status".into();
    }
    if ["sses", "shes", "ches", "xes", "zes"]
        .iter()
        .any(|suffix| value.ends_with(suffix))
    {
        return value[..value.len() - 2].into();
    }
    value.strip_suffix('s').unwrap_or(value).into()
}

/// Detect structural namespace conflicts before any generator silently allocates
/// replacement names. These diagnostics do not pretend to execute a generator.
pub fn namespace_conflicts(
    operations: &[ApiOperation],
    sources: &crate::source::Sources<'_>,
) -> Vec<(usize, usize, &'static str)> {
    use std::collections::BTreeMap;
    let mut paths: BTreeMap<Vec<String>, usize> = BTreeMap::new();
    let mut methods: BTreeMap<(Vec<String>, String), usize> = BTreeMap::new();
    for (index, operation) in operations.iter().enumerate() {
        let Some(authored) = sources.value(&operation.location.address) else {
            continue;
        };
        let path = namespace(operation, authored);
        for length in 1..=path.len() {
            paths.entry(path[..length].to_vec()).or_insert(index);
        }
        let public = public_name(operation, authored);
        let method = public.rsplit('.').next().unwrap_or("");
        if !method.is_empty() {
            methods.insert((path, method.into()), index);
        }
    }
    let mut flattened: BTreeMap<String, (Vec<String>, usize)> = BTreeMap::new();
    let mut conflicts = Vec::new();
    for (path, index) in paths {
        if let Some((child, parent)) = path.split_last()
            && let Some(previous) = methods.get(&(parent.to_vec(), child.to_ascii_lowercase()))
        {
            conflicts.push((index, *previous, "Namespace child and method occupy the same public property. Review namespace or operationId before generation."));
        }
        let key = path.concat().to_ascii_lowercase();
        if let Some((previous_path, previous)) = flattened.get(&key) {
            if previous_path != &path {
                conflicts.push((index, *previous, "Distinct namespace paths flatten to the same generated class or file name. Review namespace segments before generation."));
            }
        } else {
            flattened.insert(key, (path, index));
        }
    }
    conflicts
}
