use octri_converter::{ConversionRequest, Status, convert};
use serde_json::{Value, json};

const LIFECYCLE: &str = "EOF completes; disconnect cancels source call; errors terminate; reconnect starts a new call without replay.";

fn grpc(mapping: Value, target: &str) -> Result<ConversionRequest, serde_json::Error> {
    serde_json::from_value(json!({
        "input":{"uri":"memory://project/service.proto","content":"syntax = \"proto3\"; package events; message Empty {} message Event { string text = 1; } service Events { rpc Watch(stream Empty) returns (stream Event); }"},
        "config":{"targetVersion":target,"apiVersion":"1","httpMappings":{"events.Events.Watch":mapping}}
    }))
}

fn mapping() -> Value {
    json!({"method":"POST","path":"/events","server":"https://api.example.com","responseStatus":"200","security":[],
        "requestStream":{"framing":"ndjson","lifecycle":LIFECYCLE},
        "responseStream":{"framing":"sse","lifecycle":LIFECYCLE}
    })
}

#[test]
fn grpc_bidirectional_mapping_keeps_empty_request_messages_and_typed_sse_data()
-> Result<(), Box<dyn std::error::Error>> {
    let request = grpc(mapping(), "3.2.1")?;
    let report = convert(&request)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_ne!(report.status, Status::Blocked, "{:?}", report.decisions);
    assert!(report.mapping_requests[0].request_stream_required);
    assert!(report.mapping_requests[0].response_stream_required);
    assert_eq!(report.mapping_requests[0].key, "events.Events.Watch");
    let document = report.document.ok_or("missing output")?;
    let operation = &document["paths"]["/events"]["post"];
    assert!(
        operation
            .pointer("/requestBody/content/application~1x-ndjson/itemSchema")
            .is_some()
    );
    let data = operation
        .pointer("/responses/200/content/text~1event-stream/itemSchema/properties/data")
        .ok_or("missing SSE data model")?;
    assert_eq!(data["type"], "string");
    assert_eq!(data["contentMediaType"], "application/json");
    assert_eq!(
        data["contentSchema"]["$ref"],
        "#/components/schemas/events.Event"
    );
    assert!(
        !report
            .decisions
            .iter()
            .any(|decision| decision.code == "GRPC_STREAMING_MAPPING")
    );
    Ok(())
}

#[test]
fn older_targets_keep_raw_framing_and_require_explicit_constraint_loss_review()
-> Result<(), Box<dyn std::error::Error>> {
    for target in ["2.0", "3.0.4", "3.1.2"] {
        let mut request = grpc(mapping(), target)?;
        if target == "2.0" {
            // Isolate stream projection from unrelated ProtoJSON nullable
            // scalar unions, which Swagger cannot express without more review.
            request.input.content = Some("syntax = \"proto3\"; package events; message Empty {} service Events { rpc Watch(stream Empty) returns (stream Empty); }".into());
            request.config.base_url = Some("https://api.example.com".into());
        }
        let report = convert(&request)?;
        assert!(report.valid, "{target}: {:?}", report.diagnostics);
        assert_eq!(
            report.status,
            Status::NeedsReview,
            "{target}: {:?}",
            report.decisions
        );
        assert!(
            report
                .decisions
                .iter()
                .any(|decision| decision.code == "STREAM_SCHEMA_COMPATIBILITY"
                    && decision.review_required)
        );
        let document = report.document.ok_or("missing output")?;
        if target != "2.0" {
            let response = &document["paths"]["/events"]["post"]["responses"]["200"]["content"]["text/event-stream"];
            assert_eq!(response["schema"]["type"], "string");
            assert!(response["x-octri-stream"].get("itemSchema").is_some());
            assert!(response.get("itemSchema").is_none());
        } else {
            assert!(document.pointer("/paths/~1events/post/responses/200/x-octri-content-extensions/text~1event-stream/x-octri-stream/itemSchema").is_some());
        }
    }
    Ok(())
}

#[test]
fn incomplete_direction_or_lifecycle_cannot_complete() -> Result<(), Box<dyn std::error::Error>> {
    let mut value = mapping();
    value
        .as_object_mut()
        .ok_or("missing mapping")?
        .remove("requestStream");
    assert_eq!(convert(&grpc(value, "3.2.1")?)?.status, Status::Blocked);
    let mut value = mapping();
    value["responseStream"]["lifecycle"] = json!("");
    assert_eq!(
        convert(&grpc(value, "3.2.1")?)
            .err()
            .ok_or("accepted empty lifecycle")?
            .code,
        "HTTP_STREAM_LIFECYCLE"
    );
    Ok(())
}

#[test]
fn graphql_subscription_uses_authored_json_sequence_and_exposes_mapping_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let request: ConversionRequest = serde_json::from_value(json!({
        "input":{"uri":"memory://project/schema.graphql","content":"type Query { ping: String } type Subscription { events(topic: String!): String }"},
        "config":{"apiVersion":"1","targetVersion":"3.2.0","httpMappings":{
            "Query.ping":{"method":"GET","path":"/ping","server":"https://api.example.com","responseStatus":"200","security":[],"responseSchema":{"type":"string"}},
            "Subscription.events":{"method":"POST","path":"/events","server":"https://api.example.com","responseStatus":"200","security":[],"responseSchema":{"type":"string"},"responseStream":{"framing":"json-sequence","lifecycle":LIFECYCLE}}
        }}
    }))?;
    let report = convert(&request)?;
    assert!(report.valid, "{:?}", report.diagnostics);
    assert_eq!(report.status, Status::Complete, "{:?}", report.decisions);
    let mapping = report
        .mapping_requests
        .iter()
        .find(|request| request.key == "Subscription.events")
        .ok_or("missing mapping request")?;
    assert_eq!(mapping.input_fields, ["topic"]);
    assert!(mapping.response_stream_required);
    let document = report.document.ok_or("missing output")?;
    assert_eq!(
        document.pointer(
            "/paths/~1events/post/responses/200/content/application~1json-seq/itemSchema/type"
        ),
        Some(&json!("string"))
    );
    Ok(())
}
