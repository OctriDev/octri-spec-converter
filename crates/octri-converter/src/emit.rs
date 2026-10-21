//! Output dialect selection and explicit handling of incompatible constructs.

use crate::{Classification, Config, Result};
use crate::{
    api, schema,
    source::{Sources, child},
    trace::Trace,
};
use serde_json::{Value, json};

pub(crate) fn emit(
    document: &mut Value,
    target: &str,
    sources: &Sources,
    config: &Config,
    trace: &mut Trace,
) -> Result<()> {
    let original_version = document["openapi"].as_str().unwrap_or("").to_owned();
    let schema_default = document
        .get("jsonSchemaDialect")
        .and_then(Value::as_str)
        .and_then(octri_parser::SchemaDialect::from_uri)
        .unwrap_or_default();
    if target == "2.0" || target.starts_with("3.0.") {
        let localized_resources = schema::prepare_legacy_schemas(document, sources, config, trace)?;
        api::schemas(document, &mut |schema, pointer| {
            schema::downgrade_schema(
                schema,
                &schema::SchemaSite {
                    sources,
                    uri: &sources.root,
                    source: pointer,
                    output: pointer,
                },
                target,
                config,
                trace,
                schema_default,
                localized_resources,
            )
        });
        if localized_resources
            && let Some(dialect) = document
                .as_object_mut()
                .and_then(|map| map.remove("jsonSchemaDialect"))
        {
            trace.record(sources,&sources.root,"/jsonSchemaDialect",&[],"SCHEMA_RESOURCE_LOCALIZED",Classification::Transformed,
                "Known API schema dialect applied during migration; legacy output no longer needs a global selector",Some(&dialect));
        }
        for pointer in [
            "/webhooks",
            "/jsonSchemaDialect",
            "/info/summary",
            "/info/license/identifier",
        ] {
            incompatible(document, pointer, target, sources, config, trace);
        }
        incompatible(
            document,
            "/components/pathItems",
            target,
            sources,
            config,
            trace,
        );
    }
    if !target.starts_with("3.2.") {
        incompatible(document, "/$self", target, sources, config, trace);
        incompatible(
            document,
            "/components/mediaTypes",
            target,
            sources,
            config,
            trace,
        );
        api::structural(document, "", &mut |value, pointer, kind| {
            if !value.is_object() {
                return;
            }
            // These fields have no equivalent in older API object vocabularies.
            let fields: &[&str] = match kind {
                api::Kind::Tag => &["summary", "parent", "kind"],
                api::Kind::SecurityScheme => &["oauth2MetadataUrl", "deprecated"],
                api::Kind::PathItem => &["query", "additionalOperations"],
                api::Kind::MediaType => &["itemSchema", "prefixEncoding", "itemEncoding"],
                api::Kind::Encoding => &["encoding", "prefixEncoding", "itemEncoding"],
                api::Kind::Example => &["serializedValue", "dataValue"],
                api::Kind::Flows => &["deviceAuthorization"],
                _ => &[],
            };
            for key in fields {
                let at = child(pointer, key);
                if let Some(evidence) = value.get(*key).cloned() {
                    record_incompatible(&at, &evidence, target, sources, config, trace);
                    if config.drop_unsupported.contains(&at)
                        && let Some(map) = value.as_object_mut()
                    {
                        map.remove(*key);
                    }
                }
            }
            if kind == api::Kind::Parameter
                && value.get("in").and_then(Value::as_str) == Some("querystring")
            {
                let at = child(pointer, "in");
                record_incompatible(&at, &value["in"], target, sources, config, trace);
            }
        });
        api::schemas(document, &mut |value, pointer| {
            schema::walk_with_default(value, pointer, schema_default, &mut |schema, at| {
                if let Some(xml) = schema.get_mut("xml")
                    && let Some(evidence) = xml.get("nodeType").cloned()
                {
                    let pointer = child(&child(at, "xml"), "nodeType");
                    record_incompatible(&pointer, &evidence, target, sources, config, trace);
                    if config.drop_unsupported.contains(&pointer)
                        && let Some(map) = xml.as_object_mut()
                    {
                        map.remove("nodeType");
                    }
                }
            });
        });
    }
    if target == "2.0" {
        super::legacy::emit(document, sources, config, trace)?;
    } else {
        document["openapi"] = json!(target);
    }
    if original_version != target {
        trace.record(sources,&sources.root,"/openapi",&[if target=="2.0"{"/swagger".into()}else{"/openapi".into()}],"TARGET_DIALECT",Classification::Transformed,format!("Emit selected published dialect {target}; incompatible constructs remain blocked until resolved"),Some(&json!({"from":original_version,"to":target})));
    }
    Ok(())
}

pub(crate) fn incompatible(
    document: &mut Value,
    pointer: &str,
    target: &str,
    sources: &Sources,
    config: &Config,
    trace: &mut Trace,
) {
    if let Some(value) = document.pointer(pointer).cloned() {
        record_incompatible(pointer, &value, target, sources, config, trace);
        if config.drop_unsupported.contains(pointer)
            && let Some((parent, key)) = pointer.rsplit_once('/')
        {
            let key = key.replace("~1", "/").replace("~0", "~");
            if let Some(map) = document.pointer_mut(parent).and_then(Value::as_object_mut) {
                map.remove(&key);
            }
        }
    }
}

pub(crate) fn record_incompatible(
    pointer: &str,
    evidence: &Value,
    target: &str,
    sources: &Sources,
    config: &Config,
    trace: &mut Trace,
) {
    trace.record(sources,&sources.root,pointer,&[pointer.into()],&format!("TARGET_{target}_UNREPRESENTABLE"),if config.drop_unsupported.contains(pointer){Classification::Omitted}else{Classification::Blocked},format!("{pointer} has no automatic equivalent in {target}. Supply a replacement or explicitly approve this individual removal."),Some(evidence));
}
