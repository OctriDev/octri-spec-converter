//! Protobuf compiler uses supplied virtual files and embedded well-known types only.
use super::{mapped, text};
use crate::{Classification, Config, ConversionError, Result};
use crate::{
    source::{Sources, child},
    trace::Trace,
};
use prost_reflect::{DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MessageDescriptor};
use protox::file::{File, FileResolver, GoogleFileResolver};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

#[derive(Debug)]
struct VirtualFiles {
    files: BTreeMap<String, String>,
    known: GoogleFileResolver,
}
impl FileResolver for VirtualFiles {
    fn open_file(&self, name: &str) -> std::result::Result<File, protox::Error> {
        if let Some(text) = self.files.get(name) {
            File::from_source(name, text)
        } else {
            self.known.open_file(name)
        }
    }
}

pub(crate) fn convert(sources: &mut Sources, config: &Config, trace: &mut Trace) -> Result<Value> {
    let pool = compile(sources)?;
    trace.record(sources,&sources.root,"",&[],"PROTOJSON_REPRESENTATION",Classification::Inferred,"Generated schemas describe canonical ProtoJSON forms. Protobuf-specific numeric parsing, well-known-type bounds and alternate accepted encodings require protocol-aware runtime validation; review this boundary before publishing HTTP contracts.",None);
    let mut schemas = Map::new();
    let mut operations = Vec::new();
    for message in pool
        .all_messages()
        .filter(|message| !message.is_map_entry())
    {
        register(
            sources,
            &message.parent_file(),
            message.path(),
            &child("/messages", message.full_name()),
        );
        let schema = message_schema(&message, sources, trace)?;
        schemas.insert(message.full_name().into(), schema);
    }
    for enumeration in pool.all_enums() {
        schemas.insert(enumeration.full_name().into(),json!({"anyOf":[{"type":"string","enum":enumeration.values().map(|value|value.name().to_owned()).collect::<Vec<_>>()},{"type":"integer","format":"int32","minimum":-2147483648i64,"maximum":2147483647i64}],"x-protobuf-enum-values":enumeration.values().map(|value|(value.name().to_owned(),json!(value.number()))).collect::<Map<_,_>>()}));
    }
    for service in pool.services() {
        for method in service.methods() {
            let pointer = child("/methods", method.full_name());
            register(sources, &method.parent_file(), method.path(), &pointer);
            let key = method.full_name().to_owned();
            if (method.is_client_streaming()
                && config
                    .http_mappings
                    .get(&key)
                    .is_none_or(|mapping| mapping.request_stream.is_none()))
                || (method.is_server_streaming()
                    && config
                        .http_mappings
                        .get(&key)
                        .is_none_or(|mapping| mapping.response_stream.is_none()))
            {
                trace.record(sources,&sources.root,&pointer,&[],"GRPC_STREAMING_MAPPING",Classification::Blocked,"gRPC streaming requires an explicit HTTP framing and lifecycle mapping. A unary JSON response does not preserve streaming.",Some(&json!({"method":key,"clientStreaming":method.is_client_streaming(),"serverStreaming":method.is_server_streaming()})));
            }
            let options = method.options();
            for (extension, value) in options.extensions() {
                trace.record(sources,&sources.root,&pointer,&[],if extension.full_name()=="google.api.http"{"GOOGLE_HTTP_BINDING"}else{"PROTOBUF_CUSTOM_OPTION"},Classification::Omitted,format!("{} source annotation retained. Explicit httpMappings currently define the emitted transport; review annotation equivalence.",extension.full_name()),Some(&json!(value.to_string())));
            }
            let input = message_schema(&method.input(), sources, trace)?;
            let output = Some(
                json!({"$ref":format!("#{}",child("/components/schemas",method.output().full_name()))}),
            );
            operations.push(mapped::Operation {
                uri: sources.root.clone(),
                key,
                pointer,
                description: comment(&method.parent_file(), method.path()),
                request_streaming: method.is_client_streaming(),
                response_streaming: method.is_server_streaming(),
                payload_location_required: false,
                input: if !method.is_client_streaming()
                    && input
                        .get("properties")
                        .and_then(Value::as_object)
                        .is_some_and(|properties| properties.is_empty())
                {
                    None
                } else {
                    Some(input)
                },
                output,
            });
        }
    }
    mapped::emit(
        "Converted Protobuf service",
        None,
        Value::Object(schemas),
        operations,
        sources,
        config,
        trace,
    )
}

fn compile(sources: &mut Sources) -> Result<DescriptorPool> {
    let text = sources.text(&sources.root)?;
    if text.trim_start().starts_with('{') {
        let uri = sources.root.clone();
        let document = sources.parse(&uri, false)?;
        let descriptor = DescriptorPool::global()
            .get_message_by_name("google.protobuf.FileDescriptorSet")
            .ok_or_else(|| {
                ConversionError::new(
                    "PROTOBUF_DESCRIPTOR",
                    "Embedded descriptor schema unavailable",
                )
            })?;
        let encoded = document.to_string();
        let mut deserializer = serde_json::Deserializer::from_str(&encoded);
        let message = DynamicMessage::deserialize(descriptor, &mut deserializer)
            .map_err(|error| ConversionError::new("PROTOBUF_DESCRIPTOR", error.to_string()))?;
        deserializer
            .end()
            .map_err(|error| ConversionError::new("PROTOBUF_DESCRIPTOR", error.to_string()))?;
        use protox::prost::Message;
        return DescriptorPool::decode(message.encode_to_vec().as_slice())
            .map_err(|error| ConversionError::new("PROTOBUF_DESCRIPTOR", error.to_string()));
    }
    let root = url::Url::parse(&sources.root)
        .map_err(|error| ConversionError::new("PROTOBUF_URI", error.to_string()))?;
    let base = root
        .join(".")
        .map_err(|error| ConversionError::new("PROTOBUF_URI", error.to_string()))?;
    let mut files = BTreeMap::new();
    for (uri, source) in &sources.files {
        if !uri.ends_with(".proto") && uri != &sources.root {
            continue;
        }
        let parsed = url::Url::parse(uri)
            .map_err(|error| ConversionError::new("PROTOBUF_URI", error.to_string()))?;
        let name = base
            .make_relative(&parsed)
            .filter(|name| !name.starts_with("../") && !name.contains(['?', '#']))
            .ok_or_else(|| {
                ConversionError::new(
                    "PROTOBUF_IMPORT",
                    "Protobuf virtual sources must be under the root import directory",
                )
            })?;
        text::check_nesting(source, &sources.limits)?;
        files.insert(name, source.clone());
    }
    let root_name = base
        .make_relative(&root)
        .ok_or_else(|| ConversionError::new("PROTOBUF_URI", "Root has no import name"))?;
    let mut compiler = protox::Compiler::with_file_resolver(VirtualFiles {
        files,
        known: GoogleFileResolver::new(),
    });
    compiler
        .include_imports(true)
        .include_source_info(true)
        .open_file(root_name)
        .map_err(|error| ConversionError::new("PROTOBUF_COMPILE", format!("{error:?}")))?;
    let pool = compiler.descriptor_pool();
    if pool.all_messages().count() + pool.all_enums().count() > sources.limits.max_nodes {
        return Err(ConversionError::new(
            "LIMIT_NODES",
            "Descriptor set exceeds configured node budget",
        ));
    }
    Ok(pool)
}

fn message_schema(
    message: &MessageDescriptor,
    sources: &Sources,
    trace: &mut Trace,
) -> Result<Value> {
    if let Some(schema) = well_known(message.full_name()) {
        return Ok(schema);
    }
    let pointer = child("/messages", message.full_name());
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in message.fields() {
        let mut schema = field_schema(&field)?;
        if let Some(description) = comment(&field.parent_file(), field.path()) {
            schema["description"] = json!(description);
        }
        if field
            .field_descriptor_proto()
            .options
            .as_ref()
            .is_some_and(|options| options.deprecated())
        {
            schema["deprecated"] = json!(true);
        }
        for (extension, value) in field.options().extensions() {
            trace.record(
                sources,
                &sources.root,
                &pointer,
                &[],
                "PROTOBUF_FIELD_OPTION",
                Classification::Omitted,
                format!(
                    "{} validation/behavior option requires an authored schema mapping",
                    extension.full_name()
                ),
                Some(&json!(value.to_string())),
            );
        }
        if field.is_required() {
            required.push(field.json_name().to_owned());
        } else {
            schema = json!({"anyOf":[schema,{"type":"null"}]});
        }
        properties.insert(field.json_name().into(), schema);
    }
    let mut output = json!({"type":"object","properties":properties,"additionalProperties":false});
    if !required.is_empty() {
        output["required"] = json!(required);
    }
    let mut conjunctions = Vec::new();
    for group in message.oneofs() {
        let fields = group.fields().collect::<Vec<_>>();
        if fields.len() < 2 {
            continue;
        }
        let mut alternatives=fields.iter().map(|field|json!({"required":[field.json_name()],"properties":{field.json_name():{"not":{"type":"null"}}}})).collect::<Vec<_>>();
        alternatives.push(json!({"not":{"anyOf":alternatives.clone()}}));
        conjunctions.push(json!({"oneOf":alternatives}));
    }
    if !conjunctions.is_empty() {
        output["allOf"] = json!(conjunctions);
    }
    if let Some(description) = comment(&message.parent_file(), message.path()) {
        output["description"] = json!(description);
    }
    Ok(output)
}

fn field_schema(field: &FieldDescriptor) -> Result<Value> {
    if field.is_map() {
        let Kind::Message(entry) = field.kind() else {
            return Err(ConversionError::new(
                "PROTOBUF_MAP",
                "Invalid map descriptor",
            ));
        };
        let value = entry
            .get_field_by_name("value")
            .ok_or_else(|| ConversionError::new("PROTOBUF_MAP", "Map entry has no value"))?;
        return Ok(json!({"type":"object","additionalProperties":kind_schema(&value.kind())}));
    }
    let schema = kind_schema(&field.kind());
    Ok(if field.is_list() {
        json!({"type":"array","items":schema})
    } else {
        schema
    })
}

fn kind_schema(kind: &Kind) -> Value {
    match kind {
        Kind::Bool => json!({"type":"boolean"}),
        Kind::String => json!({"type":"string"}),
        Kind::Bytes => {
            json!({"type":"string","contentEncoding":"base64","x-protobuf-type":"bytes"})
        }
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => {
            json!({"type":"integer","format":"int32","minimum":-2147483648i64,"maximum":2147483647i64})
        }
        Kind::Uint32 | Kind::Fixed32 => {
            json!({"type":"integer","minimum":0,"maximum":4294967295u64})
        }
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
            json!({"type":"string","pattern":"^-?[0-9]+$","x-protobuf-type":"int64"})
        }
        Kind::Uint64 | Kind::Fixed64 => {
            json!({"type":"string","pattern":"^[0-9]+$","x-protobuf-type":"uint64"})
        }
        Kind::Float | Kind::Double => {
            json!({"anyOf":[{"type":"number"},{"type":"string","enum":["NaN","Infinity","-Infinity"]}]})
        }
        Kind::Message(message) => well_known(message.full_name()).unwrap_or_else(
            || json!({"$ref":format!("#{}",child("/components/schemas",message.full_name()))}),
        ),
        Kind::Enum(enumeration) => {
            if enumeration.full_name() == "google.protobuf.NullValue" {
                json!({"type":"null"})
            } else {
                json!({"$ref":format!("#{}",child("/components/schemas",enumeration.full_name()))})
            }
        }
    }
}

fn well_known(name: &str) -> Option<Value> {
    Some(match name {
        "google.protobuf.Timestamp" => {
            json!({"type":"string","format":"date-time","x-protobuf-type":name})
        }
        "google.protobuf.Duration" => {
            json!({"type":"string","pattern":"^-?[0-9]+(?:\\.[0-9]{1,9})?s$","x-protobuf-type":name})
        }
        "google.protobuf.FieldMask" => json!({"type":"string","x-protobuf-type":name}),
        "google.protobuf.Struct" => json!({"type":"object","additionalProperties":true}),
        "google.protobuf.Value" => json!({}),
        "google.protobuf.ListValue" => json!({"type":"array","items":{}}),
        "google.protobuf.Any" => {
            json!({"type":"object","properties":{"@type":{"type":"string"}},"required":["@type"],"x-protobuf-type":name})
        }
        "google.protobuf.Empty" => {
            json!({"type":"object","properties":{},"additionalProperties":false})
        }
        "google.protobuf.BoolValue" => json!({"type":["boolean","null"]}),
        "google.protobuf.StringValue" => json!({"type":["string","null"]}),
        "google.protobuf.Int32Value" => {
            json!({"type":["integer","null"],"minimum":-2147483648i64,"maximum":2147483647i64})
        }
        "google.protobuf.UInt32Value" => {
            json!({"type":["integer","null"],"minimum":0,"maximum":4294967295u64})
        }
        "google.protobuf.Int64Value" => json!({"type":["string","null"],"pattern":"^-?[0-9]+$"}),
        "google.protobuf.UInt64Value" => json!({"type":["string","null"],"pattern":"^[0-9]+$"}),
        "google.protobuf.BytesValue" => {
            json!({"type":["string","null"],"contentEncoding":"base64"})
        }
        "google.protobuf.FloatValue" | "google.protobuf.DoubleValue" => {
            json!({"anyOf":[{"type":["number","null"]},{"type":"string","enum":["NaN","Infinity","-Infinity"]}]})
        }
        _ => return None,
    })
}

fn comment(file: &prost_reflect::FileDescriptor, path: &[i32]) -> Option<String> {
    let location = file
        .file_descriptor_proto()
        .source_code_info
        .as_ref()?
        .location
        .iter()
        .find(|location| location.path == path)?;
    let text = location.leading_comments.as_deref().unwrap_or("").trim();
    if text.is_empty() {
        None
    } else {
        Some(text.into())
    }
}

fn register(
    sources: &mut Sources,
    file: &prost_reflect::FileDescriptor,
    path: &[i32],
    pointer: &str,
) {
    let span = file
        .file_descriptor_proto()
        .source_code_info
        .as_ref()
        .and_then(|info| info.location.iter().find(|location| location.path == path))
        .map(|location| location.span.as_slice());
    if let Some([line, column, ..]) = span {
        // Root operation selectors are stable; locations point to actual text.
        let root = sources.root.clone();
        if let Ok(uri) = sources.relative(&root, file.name()) {
            sources.register_text_origin(
                octri_parser::Address {
                    uri: root,
                    pointer: pointer.into(),
                },
                octri_parser::Address {
                    uri,
                    pointer: pointer.into(),
                },
                *line as usize + 1,
                *column as usize + 1,
            );
        }
    }
}
