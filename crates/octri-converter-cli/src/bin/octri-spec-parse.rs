//! Offline parser transport for hosts that cannot load the native bindings.
//! Process isolation and deadlines belong to the service's existing supervisor.
use octri_parser::{Address, Limits, NoIo, ParserError, Report, Request};
use serde::Deserialize;
use std::io::{Read, Write};

const MAX_BYTES: usize = octri_converter::MAX_ENVELOPE_BYTES;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    abi_version: u32,
    request: Request,
}

fn failure(code: &str, message: &str) -> ParserError {
    ParserError::new(code, message, Address::default())
}

fn check_limits(limits: &Limits) -> octri_parser::Result<()> {
    let maximum = Limits::default();
    let bounded = [
        (limits.max_document_bytes, maximum.max_document_bytes),
        (limits.max_total_bytes, maximum.max_total_bytes),
        (limits.max_documents, maximum.max_documents),
        (limits.max_nodes, 2_000_000),
        (limits.max_depth, maximum.max_depth),
        (limits.max_output_bytes, maximum.max_output_bytes),
        (limits.max_diagnostics, maximum.max_diagnostics),
    ];
    if bounded
        .iter()
        .any(|(value, ceiling)| *value == 0 || value > ceiling)
        || limits.max_aliases > maximum.max_aliases
    {
        return Err(failure(
            "INVALID_OPTIONS",
            "Parser limits may lower but never exceed the service ceilings",
        ));
    }
    Ok(())
}

fn execute() -> octri_parser::Result<Report> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments
        .next()
        .is_some_and(|argument| argument != "execute")
        || arguments.next().is_some()
    {
        return Err(failure(
            "INVALID_REQUEST",
            "Use execute with a parser ABI version 1 envelope on stdin",
        ));
    }
    let mut raw = String::new();
    std::io::stdin()
        .take(MAX_BYTES as u64 + 1)
        .read_to_string(&mut raw)
        .map_err(|_| failure("INVALID_REQUEST", "Request must contain valid UTF-8 JSON"))?;
    let envelope: Envelope = octri_parser::wire::decode_json("", &raw)
        .map_err(|error| failure("INVALID_REQUEST", &error.message))?;
    if envelope.abi_version != 1 {
        return Err(failure("ABI_VERSION", "Expected parser ABI version 1"));
    }
    check_limits(&envelope.request.options.limits)?;
    let resolve = &envelope.request.options.resolve;
    if resolve.http || resolve.allow_private || resolve.file_root.is_some() {
        return Err(failure(
            "HOST_CAPABILITY_DENIED",
            "Parser service accepts supplied source bytes only; file and network capabilities are unavailable",
        ));
    }
    octri_parser::execute(envelope.request, &mut NoIo)
}

fn main() {
    let result = execute();
    let mut failed = result.is_err();
    let mut output = octri_parser::wire::encode_report(result);
    if output.len() > MAX_BYTES {
        output = octri_parser::wire::encode_report(Err(failure(
            "OUTPUT_LIMIT",
            "Parser response exceeds 128 MiB",
        )));
        failed = true;
    }
    let mut stdout = std::io::stdout().lock();
    if stdout
        .write_all(output.as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush())
        .is_err()
    {
        std::process::exit(2);
    }
    std::process::exit(if failed { 2 } else { 0 });
}
