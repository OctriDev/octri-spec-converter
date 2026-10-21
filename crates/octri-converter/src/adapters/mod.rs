mod asyncapi;
pub(crate) mod common;
mod curl;
mod graphql;
mod har;
mod http;
mod http_method;
mod insomnia;
mod mapped;
mod postman;
mod protobuf;
mod raml;
mod references;
mod response_variants;
pub(crate) mod swagger;
mod text;
mod variables;
mod wsdl;
mod xml;

pub(crate) fn convert(
    format: crate::InputFormat,
    sources: &mut crate::source::Sources,
    config: &crate::Config,
    trace: &mut crate::trace::Trace,
) -> crate::Result<serde_json::Value> {
    if format == crate::InputFormat::Postman {
        return postman::convert(sources, config, trace);
    }
    if format == crate::InputFormat::Har {
        return har::convert(sources, config, trace);
    }
    if format == crate::InputFormat::Curl {
        return curl::convert(sources, config, trace);
    }
    if format == crate::InputFormat::Insomnia {
        return insomnia::convert(sources, config, trace);
    }
    if format == crate::InputFormat::Graphql {
        return graphql::convert(sources, config, trace);
    }
    if format == crate::InputFormat::Protobuf {
        return protobuf::convert(sources, config, trace);
    }
    if format == crate::InputFormat::Asyncapi {
        return asyncapi::convert(sources, config, trace);
    }
    if format == crate::InputFormat::Raml {
        return raml::convert(sources, config, trace);
    }
    if format == crate::InputFormat::Wsdl {
        return wsdl::convert(sources, config, trace);
    }
    Err(crate::ConversionError::new(
        "ADAPTER_NOT_QUALIFIED",
        format!("{format:?} adapter is not yet qualified"),
    ))
}
