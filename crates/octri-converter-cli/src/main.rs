//! Filesystem and process capabilities belong to this host, never the engine.
mod args;
mod commands;
mod files;
mod worker;

use clap::Parser;
use std::io::{Read, Write};

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Conversion exceeded its process deadline; worker terminated")]
    Timeout,
}

type Result<T> = std::result::Result<T, Error>;

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            if matches!(error, Error::Timeout) {
                3
            } else {
                2
            }
        }
    };
    std::process::exit(code);
}

fn run() -> Result<i32> {
    // Preserve stdin transport used by existing binding parity runners.
    if std::env::args_os().len() == 1 {
        return execute();
    }
    let args = args::Cli::parse();
    if matches!(args.command, args::Command::Execute) {
        execute()
    } else {
        commands::run(args)
    }
}

fn execute() -> Result<i32> {
    let mut input = String::new();
    std::io::stdin()
        .take(octri_converter::MAX_ENVELOPE_BYTES as u64 + 1)
        .read_to_string(&mut input)?;
    let output = octri_converter::wire::dispatch(&input);
    let failed = octri_parser::wire::decode_json::<serde_json::Value>("", &output)
        .map_err(|error| Error::Message(error.to_string()))?
        .get("Err")
        .is_some();
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(output.as_bytes())?;
    stdout.write_all(b"\n")?;
    Ok(if failed { 2 } else { 0 })
}
