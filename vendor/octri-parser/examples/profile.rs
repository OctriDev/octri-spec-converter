//! Native-only profiling harness separates core execution from JSON transport.
use octri_parser::{Input, NoIo, Operation, Options, Request};
use serde_json::json;
use std::time::Instant;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut document = json!({"openapi":"3.0.4","info":{"title":"Benchmark","version":"1"},"paths":{},"components":{"schemas":{"Pet":{"type":"object","properties":{"id":{"type":"string"}}}}}});
    for i in 0..500 {
        document["paths"][format!("/pets/{i}")] = json!({"get":{"operationId":format!("getPet{i}"),"responses":{"200":{"description":"OK","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Pet"}}}}}}});
    }
    let content = document.to_string();
    for operation in [Operation::Parse, Operation::Bundle, Operation::Validate] {
        let mut samples = Vec::new();
        for i in 0..8 {
            let request = Request {
                operation,
                input: Input {
                    uri: "memory://octri/openapi.yaml".into(),
                    content: Some(content.clone()),
                    generated: true,
                },
                options: Options::default(),
            };
            let started = Instant::now();
            let report = octri_parser::execute(request, &mut NoIo)?;
            let core = started.elapsed().as_secs_f64() * 1000.;
            let started = Instant::now();
            let encoded = octri_parser::wire::encode_report(Ok(report));
            let encode = started.elapsed().as_secs_f64() * 1000.;
            if i >= 3 {
                samples.push((core, encode, encoded.len()));
            }
        }
        samples.sort_by(|a, b| a.0.total_cmp(&b.0));
        let core = samples[2].0;
        samples.sort_by(|a, b| a.1.total_cmp(&b.1));
        let encode = samples[2].1;
        println!(
            "{}",
            json!({"operation":operation,"coreMs":core,"reportEncodeMs":encode,"responseBytes":samples[2].2})
        );
    }
    Ok(())
}
