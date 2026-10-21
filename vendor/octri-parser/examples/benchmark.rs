//! Standalone native measurements. No Node runtime or binding is linked.
//! Core timings exclude request construction and report encoding; CLI timings
//! include a fresh process, stdin, validation initialization and stdout.
use octri_parser::{Input, NoIo, Operation, Options, Request, ValidationMode};
use serde_json::{Value, json};
use std::{
    hint::black_box,
    io::Write,
    process::{Command, Stdio},
    time::Instant,
};

const WARMUPS: usize = 5;
const RUNS: usize = 21;
const CLI_RUNS: usize = 7;

fn specification() -> String {
    let mut document = json!({"openapi":"3.0.4","info":{"title":"Benchmark","version":"1"},"paths":{},"components":{"schemas":{"Pet":{"type":"object","properties":{"id":{"type":"string"}}}}}});
    for index in 0..500 {
        document["paths"][format!("/pets/{index}")] = json!({"get":{"operationId":format!("getPet{index}"),"responses":{"200":{"description":"OK","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Pet"}}}}}}});
    }
    document.to_string()
}

fn summary(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    let count = samples.len();
    json!({"medianMs":samples[count / 2],"p10Ms":samples[count / 10],"p90Ms":samples[count * 9 / 10],"runs":count})
}

fn measure<T: serde::Serialize>(
    execute: impl FnOnce() -> octri_parser::Result<T>,
) -> Result<(f64, f64, f64), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let report = black_box(execute()?);
    let elapsed = started.elapsed().as_secs_f64() * 1000.;
    let started = Instant::now();
    let encoded = black_box(serde_json::to_vec(&report)?);
    let encode_ms = started.elapsed().as_secs_f64() * 1000.;
    let started = Instant::now();
    drop(report);
    let drop_ms = started.elapsed().as_secs_f64() * 1000.;
    black_box(encoded);
    Ok((elapsed, encode_ms, drop_ms))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let label = std::env::args().nth(1).unwrap_or_else(|| "default".into());
    let content = specification();
    let operations = [
        ("parse", Operation::Parse),
        ("bundle", Operation::Bundle),
        ("dereference", Operation::Dereference),
        ("validate", Operation::Validate),
    ];
    let mut rows = Vec::new();
    let mut documents = Vec::new();
    let modes = [
        ("compatible", ValidationMode::Compatible),
        ("strict", ValidationMode::Strict),
    ];
    for document_only in [false, true] {
        for (mode_name, mode) in modes {
            for generated in [true, false] {
                for (name, operation) in operations {
                    let mut core = Vec::new();
                    let mut encoding = Vec::new();
                    let mut disposal = Vec::new();
                    for iteration in 0..WARMUPS + RUNS {
                        let request = Request {
                            operation,
                            input: Input {
                                uri: "memory://octri/openapi.yaml".into(),
                                content: Some(content.clone()),
                                generated,
                            },
                            options: Options {
                                validate: octri_parser::ValidationOptions {
                                    mode,
                                    ..Default::default()
                                },
                                ..Default::default()
                            },
                        };
                        let (elapsed, encode_ms, drop_ms) = if document_only {
                            measure(|| octri_parser::execute_document(request, &mut NoIo))?
                        } else {
                            measure(|| octri_parser::execute(request, &mut NoIo))?
                        };
                        if iteration >= WARMUPS {
                            core.push(elapsed);
                            encoding.push(encode_ms);
                            disposal.push(drop_ms);
                        }
                    }
                    let row = json!({"mode":mode_name,"operation":name,"inputKind":if generated {"generated-json"} else {"json-text"},"core":summary(core),"fullReportEncoding":summary(encoding),"reportDrop":summary(disposal)});
                    if document_only {
                        documents.push(row);
                    } else {
                        rows.push(row);
                    }
                }
            }
        }
    }
    let binary = "target/release/octri-openapi";
    let mut cli = Vec::new();
    for (mode_name, mode) in modes {
        for (operation, _) in operations {
            let mut samples = Vec::new();
            let mut bytes = 0;
            for iteration in 0..CLI_RUNS + 1 {
                let started = Instant::now();
                let mut command = Command::new(binary);
                command.args([operation, "-", "--base-uri", "memory://octri/openapi.yaml"]);
                if mode == ValidationMode::Strict {
                    command.arg("--strict");
                }
                let mut child = command
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()?;
                child
                    .stdin
                    .take()
                    .ok_or("CLI stdin unavailable")?
                    .write_all(content.as_bytes())?;
                let output = child.wait_with_output()?;
                let elapsed = started.elapsed().as_secs_f64() * 1000.;
                if !output.status.success() {
                    return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
                }
                bytes = output.stdout.len();
                if iteration > 0 {
                    samples.push(elapsed);
                }
            }
            cli.push(
            json!({"mode":mode_name,"operation":operation,"freshProcess":summary(samples),"stdoutBytes":bytes}),
        );
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"label":label,"platform":std::env::consts::OS,"arch":std::env::consts::ARCH,"profile":"release","nodeBindings":false,"operations":500,"bytes":content.len(),"warmups":WARMUPS,"coreResults":rows,"documentResults":documents,"cliResults":cli,"notes":"coreResults returns full source-aware Report; documentResults returns DocumentOutput, with text positions replayed only on errors. Encoding/drop measured separately; the encoding column retains its historical name fullReportEncoding for both output kinds. CLI includes process start, stdin, cold per-process validators, transformation and stdout. CLI does not emit full reports. Phase medians are independent."})
        )?
    );
    Ok(())
}
