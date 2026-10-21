//! Caller-authored framing; standard item schemas when the target supports them.
use crate::{Classification, Config, ConversionError, Result, StreamBinding, StreamFraming};
use crate::{
    source::{Sources, child},
    trace::Trace,
};
use serde_json::{Value, json};

#[allow(clippy::too_many_arguments)]
pub(super) fn content(
    schema: Value,
    stream: Option<&StreamBinding>,
    config: &Config,
    uri: &str,
    pointer: &str,
    output: &str,
    sources: &Sources,
    trace: &mut Trace,
) -> Result<Value> {
    let Some(stream) = stream else {
        return Ok(json!({"application/json":{"schema":schema}}));
    };
    if stream.lifecycle.trim().is_empty() || stream.lifecycle.len() > 4096 {
        return Err(ConversionError::new(
            "HTTP_STREAM_LIFECYCLE",
            "Stream lifecycle must describe completion, cancellation, errors and resumption within 4096 bytes",
        ));
    }
    let (media, item) = match stream.framing {
        StreamFraming::Ndjson => ("application/x-ndjson", schema),
        StreamFraming::JsonSequence => ("application/json-seq", schema),
        StreamFraming::Sse => (
            "text/event-stream",
            json!({
                "type":"object","required":["data"],"properties":{
                    "data":{"type":"string","contentMediaType":"application/json","contentSchema":schema},
                    "event":{"type":"string"},"id":{"type":"string"},"retry":{"type":"integer","minimum":0}
                }
            }),
        ),
    };
    let at = child(&child(output, "content"), media);
    let metadata = json!({"framing":stream.framing,"lifecycle":stream.lifecycle});
    let payload = if config
        .target_version
        .as_deref()
        .is_some_and(|version| version.starts_with("3.2."))
    {
        json!({"itemSchema":item,"x-octri-stream":metadata})
    } else {
        trace.record(sources,uri,pointer,std::slice::from_ref(&at),"STREAM_SCHEMA_COMPATIBILITY",Classification::Omitted,"Selected target lacks standard streaming itemSchema. Raw stream remains a string; message schema and lifecycle remain in x-octri-stream. Choose OpenAPI 3.2 to expose standard item constraints.",Some(&metadata));
        json!({"schema":{"type":"string"},"x-octri-stream":{"framing":stream.framing,"lifecycle":stream.lifecycle,"itemSchema":item}})
    };
    trace.record(sources,uri,pointer,&[at],"EXPLICIT_STREAM_MAPPING",Classification::Transformed,"Caller-authored HTTP stream framing and lifecycle applied. This describes a gateway contract; conversion does not implement protocol transport.",Some(&metadata));
    Ok(json!({media:payload}))
}
