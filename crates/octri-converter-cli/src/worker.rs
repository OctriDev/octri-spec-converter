//! Isolate engine CPU and memory lifetime from the CLI host.
use crate::{Error, Result};
use serde_json::Value;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) fn call(request: &Value, timeout_ms: u64) -> Result<Value> {
    let input = serde_json::to_vec(request)?;
    if input.len() > octri_converter::MAX_ENVELOPE_BYTES {
        return Err(Error::Message(
            "Request exceeds 128 MiB transport limit".into(),
        ));
    }
    let mut child = Command::new(std::env::current_exe()?)
        .arg("execute")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| Error::Message("Worker stdin unavailable".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Message("Worker stdout unavailable".into()))?;
    // Drain both pipes concurrently. Large envelopes must not deadlock before
    // the parent can enforce the process deadline.
    let writer = thread::spawn(move || {
        let mut stdin = stdin;
        stdin.write_all(&input)
    });
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout
            .take(octri_converter::MAX_ENVELOPE_BYTES as u64 + 1)
            .read_to_end(&mut output)?;
        Ok::<_, std::io::Error>(output)
    });
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let timed_out = loop {
        if child.try_wait()?.is_some() {
            break false;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            break true;
        }
        thread::sleep(Duration::from_millis(2));
    };
    let written = writer
        .join()
        .map_err(|_| Error::Message("Worker input thread failed".into()))?;
    let output = reader
        .join()
        .map_err(|_| Error::Message("Worker output thread failed".into()))??;
    if timed_out {
        return Err(Error::Timeout);
    }
    written?;
    if output.len() > octri_converter::MAX_ENVELOPE_BYTES {
        return Err(Error::Message(
            "Worker exceeded output transport limit".into(),
        ));
    }
    let text = std::str::from_utf8(&output).map_err(|error| Error::Message(error.to_string()))?;
    let result: Value = octri_parser::wire::decode_json("", text)
        .map_err(|error| Error::Message(error.to_string()))?;
    if result.as_object().is_none_or(|value| value.len() != 1)
        || (result.get("Ok").is_none() && result.get("Err").is_none())
    {
        return Err(Error::Message(
            "Worker returned an invalid result envelope".into(),
        ));
    }
    Ok(result)
}
