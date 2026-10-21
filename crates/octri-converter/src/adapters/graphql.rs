//! GraphQL SDL and introspection share one typed schema projection.
use super::mapped;
use crate::{Classification, Config, ConversionError, Result};
use crate::{
    source::{Sources, child},
    trace::Trace,
};
use graphql_parser::schema as gql;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

mod validation;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TypeRef {
    kind: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    of_type: Option<Box<TypeRef>>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InputValue {
    name: String,
    description: Option<String>,
    #[serde(rename = "type")]
    ty: TypeRef,
    #[serde(default)]
    default_value: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Field {
    name: String,
    description: Option<String>,
    #[serde(rename = "type")]
    ty: TypeRef,
    #[serde(default)]
    args: Vec<InputValue>,
    #[serde(default)]
    is_deprecated: bool,
    #[serde(default)]
    deprecation_reason: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
struct EnumValue {
    name: String,
    description: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Type {
    name: String,
    kind: String,
    description: Option<String>,
    #[serde(default)]
    fields: Option<Vec<Field>>,
    #[serde(default)]
    input_fields: Option<Vec<InputValue>>,
    #[serde(default)]
    enum_values: Option<Vec<EnumValue>>,
    #[serde(default)]
    possible_types: Option<Vec<TypeRef>>,
    #[serde(default)]
    interfaces: Option<Vec<TypeRef>>,
}
#[derive(Debug)]
struct Schema {
    types: BTreeMap<String, Type>,
    roots: Vec<(String, String)>,
    pointers: BTreeMap<String, String>,
}

pub(crate) fn convert(sources: &mut Sources, config: &Config, trace: &mut Trace) -> Result<Value> {
    let uri = sources.root.clone();
    let raw = sources.text(&uri)?.trim_start();
    let schema = if raw.starts_with('{') {
        let document = sources.parse(&uri, false)?;
        introspection(&document)?
    } else {
        sdl(sources, trace)?
    };
    validation::validate(&schema)?;
    let mut schemas = Map::new();
    let mut operations = Vec::new();
    for (name, definition) in &schema.types {
        if name.starts_with("__") || builtin(name) {
            continue;
        }
        let at = schema
            .pointers
            .get(name)
            .cloned()
            .unwrap_or_else(|| child("/types", name));
        let mut output = match definition.kind.as_str() {
            "SCALAR" => {
                trace.record(sources,&uri,&at,&[child("/components/schemas",name)],"GRAPHQL_CUSTOM_SCALAR",Classification::Inferred,"Custom scalar serialization/validation is not defined by its name. Supply an authored schema or review unconstrained representation.",Some(&json!(name)));
                json!({})
            }
            "ENUM" => {
                let values = definition.enum_values.as_deref().unwrap_or_default();
                let mut names = BTreeSet::new();
                for value in values {
                    if !names.insert(&value.name) {
                        return Err(ConversionError::new("GRAPHQL_ENUM", "Duplicate enum value"));
                    }
                }
                let mut result = json!({"type":"string","enum":values.iter().map(|value|&value.name).collect::<Vec<_>>()});
                let descriptions = values
                    .iter()
                    .filter_map(|value| {
                        value
                            .description
                            .as_ref()
                            .map(|description| (value.name.clone(), json!(description)))
                    })
                    .collect::<Map<_, _>>();
                if !descriptions.is_empty() {
                    result["x-graphql-enum-descriptions"] = Value::Object(descriptions);
                }
                result
            }
            "UNION" | "INTERFACE" => {
                let mut possibilities = definition.possible_types.clone().unwrap_or_default();
                if possibilities.is_empty() && definition.kind == "INTERFACE" {
                    possibilities = schema
                        .types
                        .values()
                        .filter(|ty| {
                            ty.kind == "OBJECT"
                                && ty
                                    .interfaces
                                    .as_deref()
                                    .unwrap_or_default()
                                    .iter()
                                    .any(|interface| interface.name.as_deref() == Some(name))
                        })
                        .map(|ty| TypeRef {
                            kind: "OBJECT".into(),
                            name: Some(ty.name.clone()),
                            of_type: None,
                        })
                        .collect();
                }
                if possibilities.is_empty() {
                    trace.record(
                        sources,
                        &uri,
                        &at,
                        &[],
                        "GRAPHQL_ABSTRACT_TYPE",
                        Classification::Blocked,
                        "Abstract type has no declared concrete implementations",
                        Some(&json!(name)),
                    );
                    json!({})
                } else {
                    json!({"anyOf":possibilities.iter().map(|reference|reference_schema(reference,&schema.types,false)).collect::<Result<Vec<_>>>()?})
                }
            }
            "OBJECT" => {
                let mut properties = Map::new();
                for field in definition.fields.as_deref().unwrap_or_default() {
                    let mut value = reference_schema(&field.ty, &schema.types, true)?;
                    if let Some(description) = &field.description {
                        value["description"] = json!(description);
                    }
                    if field.is_deprecated {
                        value["deprecated"] = json!(true);
                        if let Some(reason) = &field.deprecation_reason {
                            value["x-graphql-deprecation-reason"] = json!(reason);
                        }
                    }
                    if properties.insert(field.name.clone(), value).is_some() {
                        return Err(ConversionError::new(
                            "GRAPHQL_FIELD",
                            "Duplicate object field",
                        ));
                    }
                    if !field.args.is_empty() && !schema.roots.iter().any(|(_, root)| root == name)
                    {
                        trace.record(sources,&uri,&child(&child(&at,"fields"),&field.name),&[],"GRAPHQL_NESTED_FIELD_ARGUMENTS",Classification::Omitted,"Nested field arguments require an authored HTTP response projection; type shape alone cannot express resolver execution",Some(&json!(field.name)));
                    }
                }
                json!({"type":"object","properties":properties})
            }
            "INPUT_OBJECT" => input_schema(
                definition.input_fields.as_deref().unwrap_or_default(),
                &schema.types,
            )?,
            kind => {
                return Err(ConversionError::new(
                    "GRAPHQL_TYPE",
                    format!("Unknown GraphQL kind {kind}"),
                ));
            }
        };
        if let Some(description) = &definition.description {
            output["description"] = json!(description);
        }
        schemas.insert(name.clone(), output);
    }
    for (operation_kind, root_name) in &schema.roots {
        let root = schema
            .types
            .get(root_name)
            .filter(|ty| ty.kind == "OBJECT")
            .ok_or_else(|| {
                ConversionError::new(
                    "GRAPHQL_ROOT",
                    format!("Root {root_name} must name a declared object"),
                )
            })?;
        for field in root.fields.as_deref().unwrap_or_default() {
            let key = format!("{root_name}.{}", field.name);
            let pointer = schema
                .pointers
                .get(&key)
                .cloned()
                .unwrap_or_else(|| child(&child("/types", root_name), &field.name));
            let input = input_schema(&field.args, &schema.types)?;
            if operation_kind == "subscription"
                && config
                    .http_mappings
                    .get(&key)
                    .is_some_and(|mapping| mapping.response_stream.is_none())
            {
                trace.record(sources,&uri,&pointer,&[],"GRAPHQL_SUBSCRIPTION_TRANSPORT",Classification::Blocked,"Subscription requires an authored streaming HTTP representation. A single JSON response cannot preserve subscription semantics.",Some(&json!(key)));
            }
            if config
                .http_mappings
                .get(&key)
                .is_some_and(|mapping| mapping.response_schema.is_none())
            {
                trace.record(sources,&uri,&pointer,&[],"GRAPHQL_RESPONSE_PROJECTION",Classification::Inferred,"Generated response describes possible selected fields, all optional. Supply responseSchema to define a fixed HTTP projection and GraphQL error envelope.",Some(&json!(key)));
            }
            operations.push(mapped::Operation {
                uri: uri.clone(),
                key,
                pointer,
                description: field.description.clone(),
                input: if field.args.is_empty() {
                    None
                } else {
                    Some(input)
                },
                request_streaming: false,
                response_streaming: operation_kind == "subscription",
                payload_location_required: false,
                output: Some(reference_schema(&field.ty, &schema.types, true)?),
            });
        }
    }
    mapped::emit(
        "Converted GraphQL schema",
        None,
        Value::Object(schemas),
        operations,
        sources,
        config,
        trace,
    )
}

fn builtin(name: &str) -> bool {
    matches!(name, "Int" | "Float" | "String" | "Boolean" | "ID")
}

fn reference_schema(
    reference: &TypeRef,
    types: &BTreeMap<String, Type>,
    nullable: bool,
) -> Result<Value> {
    if reference.kind == "NON_NULL" {
        return reference_schema(
            reference
                .of_type
                .as_deref()
                .ok_or_else(|| ConversionError::new("GRAPHQL_TYPE", "NON_NULL needs ofType"))?,
            types,
            false,
        );
    }
    let schema = if reference.kind == "LIST" {
        json!({"type":"array","items":reference_schema(reference.of_type.as_deref().ok_or_else(||ConversionError::new("GRAPHQL_TYPE","LIST needs ofType"))?,types,true)?})
    } else {
        let name = reference
            .name
            .as_deref()
            .ok_or_else(|| ConversionError::new("GRAPHQL_TYPE", "Named type requires name"))?;
        match name {
            "Int" => {
                json!({"type":"integer","format":"int32","minimum":-2147483648i64,"maximum":2147483647i64})
            }
            "Float" => json!({"type":"number"}),
            "String" | "ID" => json!({"type":"string"}),
            "Boolean" => json!({"type":"boolean"}),
            name if types.contains_key(name) => {
                json!({"$ref":format!("#{}",child("/components/schemas",name))})
            }
            name => {
                return Err(ConversionError::new(
                    "GRAPHQL_TYPE",
                    format!("Unknown type {name}"),
                ));
            }
        }
    };
    Ok(if nullable {
        json!({"anyOf":[schema,{"type":"null"}]})
    } else {
        schema
    })
}

fn input_schema(fields: &[InputValue], types: &BTreeMap<String, Type>) -> Result<Value> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in fields {
        let mut schema = reference_schema(&field.ty, types, true)?;
        if let Some(description) = &field.description {
            schema["description"] = json!(description);
        }
        if let Some(default) = &field.default_value {
            let query_source = format!("query($value: String = {default}) {{ __typename }}");
            let value = graphql_parser::parse_query::<String>(&query_source)
                .map_err(|error| ConversionError::new("GRAPHQL_DEFAULT", error.to_string()))?;
            if let Some(graphql_parser::query::Definition::Operation(
                graphql_parser::query::OperationDefinition::Query(query),
            )) = value.definitions.first()
                && let Some(default) = query
                    .variable_definitions
                    .first()
                    .and_then(|variable| variable.default_value.as_ref())
            {
                schema["default"] = literal(default)?;
            }
        }
        if field.ty.kind == "NON_NULL" && field.default_value.is_none() {
            required.push(field.name.clone());
        }
        if properties.insert(field.name.clone(), schema).is_some() {
            return Err(ConversionError::new(
                "GRAPHQL_INPUT",
                "Duplicate input field",
            ));
        }
    }
    let mut result = json!({"type":"object","properties":properties,"additionalProperties":false});
    if !required.is_empty() {
        result["required"] = json!(required);
    }
    Ok(result)
}

fn literal(value: &gql::Value<'_, String>) -> Result<Value> {
    Ok(match value {
        gql::Value::Variable(_) => {
            return Err(ConversionError::new(
                "GRAPHQL_DEFAULT",
                "Defaults cannot contain variables",
            ));
        }
        gql::Value::Int(number) => json!(
            number
                .as_i64()
                .ok_or_else(|| ConversionError::new("GRAPHQL_DEFAULT", "Invalid integer"))?
        ),
        gql::Value::Float(number) => json!(number),
        gql::Value::String(value) | gql::Value::Enum(value) => json!(value),
        gql::Value::Boolean(value) => json!(value),
        gql::Value::Null => Value::Null,
        gql::Value::List(values) => {
            Value::Array(values.iter().map(literal).collect::<Result<Vec<_>>>()?)
        }
        gql::Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| Ok((key.clone(), literal(value)?)))
                .collect::<Result<Map<_, _>>>()?,
        ),
    })
}

fn introspection(document: &Value) -> Result<Schema> {
    let (root, prefix) = if let Some(root) = document.get("__schema") {
        (root, "/__schema")
    } else {
        (
            document.pointer("/data/__schema").ok_or_else(|| {
                ConversionError::new(
                    "GRAPHQL_INTROSPECTION",
                    "Expected __schema introspection result",
                )
            })?,
            "/data/__schema",
        )
    };
    let mut types = BTreeMap::new();
    let mut pointers = BTreeMap::new();
    for (index, value) in root["types"]
        .as_array()
        .ok_or_else(|| {
            ConversionError::new("GRAPHQL_INTROSPECTION", "Introspection needs types array")
        })?
        .iter()
        .enumerate()
    {
        let ty: Type = serde_json::from_value(value.clone())
            .map_err(|error| ConversionError::new("GRAPHQL_INTROSPECTION", error.to_string()))?;
        let pointer = child(&child(prefix, "types"), &index.to_string());
        for (index, field) in ty.fields.as_deref().unwrap_or_default().iter().enumerate() {
            pointers.insert(
                format!("{}.{}", ty.name, field.name),
                child(&child(&pointer, "fields"), &index.to_string()),
            );
        }
        pointers.insert(ty.name.clone(), pointer);
        if types.insert(ty.name.clone(), ty).is_some() {
            return Err(ConversionError::new("GRAPHQL_TYPE", "Duplicate named type"));
        }
    }
    let mut roots = Vec::new();
    for (kind, key) in [
        ("query", "queryType"),
        ("mutation", "mutationType"),
        ("subscription", "subscriptionType"),
    ] {
        if let Some(name) = root[key]["name"].as_str() {
            roots.push((kind.into(), name.into()));
        }
    }
    if roots.is_empty() {
        return Err(ConversionError::new(
            "GRAPHQL_ROOT",
            "Introspection has no operation roots",
        ));
    }
    Ok(Schema {
        types,
        roots,
        pointers,
    })
}

fn sdl(sources: &mut Sources, trace: &mut Trace) -> Result<Schema> {
    let uri = sources.root.clone();
    let text = sources.text(&uri)?.to_owned();
    super::text::check_nesting(&text, &sources.limits)?;
    let document = gql::parse_schema::<String>(&text)
        .map_err(|error| ConversionError::new("GRAPHQL_SYNTAX", error.to_string()))?;
    let mut types = BTreeMap::new();
    let mut roots = None;
    let mut pointers = BTreeMap::new();
    let mut extensions = Vec::new();
    for definition in &document.definitions {
        match definition {
            gql::Definition::SchemaDefinition(schema) => {
                if roots.is_some() {
                    return Err(ConversionError::new(
                        "GRAPHQL_ROOT",
                        "Duplicate schema declaration",
                    ));
                }
                roots = Some(
                    [
                        ("query", &schema.query),
                        ("mutation", &schema.mutation),
                        ("subscription", &schema.subscription),
                    ]
                    .into_iter()
                    .filter_map(|(kind, name)| {
                        name.as_ref().map(|name| (kind.into(), name.clone()))
                    })
                    .collect::<Vec<_>>(),
                );
                directives(&schema.directives, "/schema", sources, trace);
            }
            gql::Definition::TypeDefinition(definition) => {
                let ty = definition_type(definition, sources, trace, &mut pointers)?;
                if types.insert(ty.name.clone(), ty).is_some() {
                    return Err(ConversionError::new(
                        "GRAPHQL_TYPE",
                        "Duplicate type declaration",
                    ));
                }
            }
            gql::Definition::TypeExtension(extension) => extensions.push(extension),
            gql::Definition::DirectiveDefinition(directive) => {
                let pointer = child("/directives", &directive.name);
                sources.register_text_location(
                    &uri,
                    &pointer,
                    directive.position.line,
                    directive.position.column,
                );
                trace.record(sources,&uri,&pointer,&[],"GRAPHQL_DIRECTIVE_DEFINITION",Classification::Omitted,"Directive definition retained in source; OpenAPI does not execute GraphQL directives",Some(&json!(directive.name)));
            }
        }
    }
    for extension in extensions {
        let definition = match extension {
            gql::TypeExtension::Scalar(value) => gql::TypeDefinition::Scalar(gql::ScalarType {
                position: value.position,
                description: None,
                name: value.name.clone(),
                directives: value.directives.clone(),
            }),
            gql::TypeExtension::Object(value) => gql::TypeDefinition::Object(gql::ObjectType {
                position: value.position,
                description: None,
                name: value.name.clone(),
                directives: value.directives.clone(),
                fields: value.fields.clone(),
                implements_interfaces: value.implements_interfaces.clone(),
            }),
            gql::TypeExtension::Interface(value) => {
                gql::TypeDefinition::Interface(gql::InterfaceType {
                    position: value.position,
                    description: None,
                    name: value.name.clone(),
                    directives: value.directives.clone(),
                    fields: value.fields.clone(),
                    implements_interfaces: value.implements_interfaces.clone(),
                })
            }
            gql::TypeExtension::Union(value) => gql::TypeDefinition::Union(gql::UnionType {
                position: value.position,
                description: None,
                name: value.name.clone(),
                directives: value.directives.clone(),
                types: value.types.clone(),
            }),
            gql::TypeExtension::Enum(value) => gql::TypeDefinition::Enum(gql::EnumType {
                position: value.position,
                description: None,
                name: value.name.clone(),
                directives: value.directives.clone(),
                values: value.values.clone(),
            }),
            gql::TypeExtension::InputObject(value) => {
                gql::TypeDefinition::InputObject(gql::InputObjectType {
                    position: value.position,
                    description: None,
                    name: value.name.clone(),
                    directives: value.directives.clone(),
                    fields: value.fields.clone(),
                })
            }
        };
        let extra = definition_type(&definition, sources, trace, &mut pointers)?;
        let original = types.get_mut(&extra.name).ok_or_else(|| {
            ConversionError::new("GRAPHQL_EXTENSION", "Extension target is not declared")
        })?;
        if original.kind != extra.kind {
            return Err(ConversionError::new(
                "GRAPHQL_EXTENSION",
                "Extension kind differs from original type",
            ));
        }
        original
            .fields
            .get_or_insert_default()
            .extend(extra.fields.unwrap_or_default());
        original
            .input_fields
            .get_or_insert_default()
            .extend(extra.input_fields.unwrap_or_default());
        original
            .enum_values
            .get_or_insert_default()
            .extend(extra.enum_values.unwrap_or_default());
        original
            .possible_types
            .get_or_insert_default()
            .extend(extra.possible_types.unwrap_or_default());
        original
            .interfaces
            .get_or_insert_default()
            .extend(extra.interfaces.unwrap_or_default());
    }
    let roots = roots.unwrap_or_else(|| {
        [
            ("query", "Query"),
            ("mutation", "Mutation"),
            ("subscription", "Subscription"),
        ]
        .into_iter()
        .filter(|(_, name)| types.contains_key(*name))
        .map(|(kind, name)| (kind.into(), name.into()))
        .collect()
    });
    if !roots.iter().any(|(kind, _)| kind == "query") {
        return Err(ConversionError::new(
            "GRAPHQL_ROOT",
            "GraphQL schema requires query root",
        ));
    }
    Ok(Schema {
        types,
        roots,
        pointers,
    })
}

fn definition_type(
    definition: &gql::TypeDefinition<'_, String>,
    sources: &mut Sources,
    trace: &mut Trace,
    pointers: &mut BTreeMap<String, String>,
) -> Result<Type> {
    let (name, kind, description, position, directive_values) = match definition {
        gql::TypeDefinition::Scalar(value) => (
            &value.name,
            "SCALAR",
            &value.description,
            value.position,
            &value.directives,
        ),
        gql::TypeDefinition::Object(value) => (
            &value.name,
            "OBJECT",
            &value.description,
            value.position,
            &value.directives,
        ),
        gql::TypeDefinition::Interface(value) => (
            &value.name,
            "INTERFACE",
            &value.description,
            value.position,
            &value.directives,
        ),
        gql::TypeDefinition::Union(value) => (
            &value.name,
            "UNION",
            &value.description,
            value.position,
            &value.directives,
        ),
        gql::TypeDefinition::Enum(value) => (
            &value.name,
            "ENUM",
            &value.description,
            value.position,
            &value.directives,
        ),
        gql::TypeDefinition::InputObject(value) => (
            &value.name,
            "INPUT_OBJECT",
            &value.description,
            value.position,
            &value.directives,
        ),
    };
    let uri = sources.root.clone();
    let pointer = child("/types", name);
    sources.register_text_location(&uri, &pointer, position.line, position.column);
    pointers.insert(name.clone(), pointer.clone());
    directives(directive_values, &pointer, sources, trace);
    let mut ty = Type {
        name: name.clone(),
        kind: kind.into(),
        description: description.clone(),
        fields: None,
        input_fields: None,
        enum_values: None,
        possible_types: None,
        interfaces: None,
    };
    let fields = match definition {
        gql::TypeDefinition::Object(value) => {
            ty.interfaces = Some(
                value
                    .implements_interfaces
                    .iter()
                    .map(|name| named(name, "INTERFACE"))
                    .collect(),
            );
            Some(&value.fields)
        }
        gql::TypeDefinition::Interface(value) => Some(&value.fields),
        _ => None,
    };
    if let Some(fields) = fields {
        ty.fields = Some(
            fields
                .iter()
                .map(|field| {
                    let field_pointer = child(&child(&pointer, "fields"), &field.name);
                    sources.register_text_location(
                        &uri,
                        &field_pointer,
                        field.position.line,
                        field.position.column,
                    );
                    pointers.insert(format!("{name}.{}", field.name), field_pointer.clone());
                    directives(&field.directives, &field_pointer, sources, trace);
                    let reason = field
                        .directives
                        .iter()
                        .find(|directive| directive.name == "deprecated")
                        .and_then(|directive| {
                            directive
                                .arguments
                                .iter()
                                .find(|(name, _)| name == "reason")
                        })
                        .and_then(|(_, value)| {
                            if let gql::Value::String(value) = value {
                                Some(value.clone())
                            } else {
                                None
                            }
                        });
                    Field {
                        name: field.name.clone(),
                        description: field.description.clone(),
                        ty: type_ref(&field.field_type),
                        args: field.arguments.iter().map(input_value).collect(),
                        is_deprecated: field
                            .directives
                            .iter()
                            .any(|directive| directive.name == "deprecated"),
                        deprecation_reason: reason,
                    }
                })
                .collect(),
        );
    }
    match definition {
        gql::TypeDefinition::InputObject(value) => {
            ty.input_fields = Some(value.fields.iter().map(input_value).collect())
        }
        gql::TypeDefinition::Enum(value) => {
            ty.enum_values = Some(
                value
                    .values
                    .iter()
                    .map(|value| EnumValue {
                        name: value.name.clone(),
                        description: value.description.clone(),
                    })
                    .collect(),
            )
        }
        gql::TypeDefinition::Union(value) => {
            ty.possible_types = Some(
                value
                    .types
                    .iter()
                    .map(|name| named(name, "OBJECT"))
                    .collect(),
            )
        }
        _ => {}
    }
    Ok(ty)
}

fn named(name: &str, kind: &str) -> TypeRef {
    TypeRef {
        kind: kind.into(),
        name: Some(name.into()),
        of_type: None,
    }
}
fn type_ref(ty: &gql::Type<'_, String>) -> TypeRef {
    match ty {
        gql::Type::NamedType(name) => named(name, "NAMED"),
        gql::Type::ListType(inner) => TypeRef {
            kind: "LIST".into(),
            name: None,
            of_type: Some(Box::new(type_ref(inner))),
        },
        gql::Type::NonNullType(inner) => TypeRef {
            kind: "NON_NULL".into(),
            name: None,
            of_type: Some(Box::new(type_ref(inner))),
        },
    }
}
fn input_value(value: &gql::InputValue<'_, String>) -> InputValue {
    InputValue {
        name: value.name.clone(),
        description: value.description.clone(),
        ty: type_ref(&value.value_type),
        default_value: value.default_value.as_ref().map(ToString::to_string),
    }
}
fn directives(
    values: &[gql::Directive<'_, String>],
    pointer: &str,
    sources: &Sources,
    trace: &mut Trace,
) {
    for directive in values {
        if directive.name == "deprecated" {
            continue;
        }
        trace.record(sources,&sources.root,pointer,&[],"GRAPHQL_DIRECTIVE",Classification::Omitted,format!("@{} directive has no executable OpenAPI equivalent. Review its validation, resolver and authorization effects explicitly.",directive.name),Some(&json!(directive.to_string())));
    }
}
