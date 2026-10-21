//! Human commands share the exact versioned engine envelope used by bindings.
use crate::{Error, Result, args::*, files, worker};
use octri_converter::{ConversionReport, ConversionRequest};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

pub(crate) fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Execute => Err(Error::Message("Execute must use stdin transport".into())),
        Command::Formats => {
            stdout(&call(json!({"action":"formats"}), cli.timeout_ms)?)?;
            Ok(0)
        }
        Command::Detect(source) => {
            let mut inputs = files::Inputs::new(&source.settings)?;
            let request = inputs.request(&source.input)?;
            stdout(&call(
                json!({"action":"detect","request":request}),
                cli.timeout_ms,
            )?)?;
            Ok(0)
        }
        Command::Convert(args) => convert(args, false, cli.timeout_ms),
        Command::Bundle(args) => convert(args, true, cli.timeout_ms),
        Command::Review(args) => review(args, cli.timeout_ms),
        Command::Batch(args) => batch(args, cli.timeout_ms),
        Command::Compare {
            before,
            after,
            output,
        } => {
            let protected = BTreeSet::from([before.canonicalize()?, after.canonicalize()?]);
            let result = call(
                json!({"action":"compare","before":files::json(&before)?,"after":files::json(&after)?}),
                cli.timeout_ms,
            )?;
            deliver(&output, &result, &protected)?;
            Ok(0)
        }
        Command::Export {
            report,
            format,
            output,
        } => {
            let protected = BTreeSet::from([report.canonicalize()?]);
            let result = call(
                json!({"action":"export","report":files::json(&report)?,"format":format.as_str()}),
                cli.timeout_ms,
            )?;
            if matches!(format, ExportFormat::Artifact) {
                if output.force {
                    return Err(Error::Message(
                        "Artifact directories must be new; --force is not accepted".into(),
                    ));
                }
                let destination = output.out.ok_or_else(|| {
                    Error::Message("Artifact export requires --out NEW_DIRECTORY".into())
                })?;
                publish_artifact(&destination, &result)?;
            } else if let Some(html) = result.as_str() {
                deliver_bytes(&output, html.as_bytes(), &protected)?;
            } else {
                deliver(&output, &result, &protected)?;
            }
            Ok(0)
        }
    }
}

fn call(mut envelope: Value, timeout_ms: u64) -> Result<Value> {
    envelope["abiVersion"] = json!(1);
    let result = worker::call(&envelope, timeout_ms)?;
    if let Some(error) = result.get("Err") {
        return Err(Error::Message(serde_json::to_string(error)?));
    }
    result
        .get("Ok")
        .cloned()
        .ok_or_else(|| Error::Message("Worker result is missing".into()))
}

fn convert(args: Convert, bundle: bool, timeout_ms: u64) -> Result<i32> {
    let mut inputs = files::Inputs::new(&args.source.settings)?;
    let mut request = inputs.request(&args.source.input)?;
    request.config.bundle |= bundle;
    let report = call(json!({"action":"convert","request":request}), timeout_ms)?;
    if let (Some(out), Some(report_path)) = (&args.output.out, &args.report)
        && output_identity(out)? == output_identity(report_path)?
    {
        return Err(Error::Message(
            "Converted output and report must have different paths".into(),
        ));
    }
    if let Some(path) = &args.report {
        files::write(
            path,
            &serde_json::to_vec_pretty(&report)?,
            args.output.force,
            &inputs.protected,
        )?;
    }
    let content = report
        .pointer("/output/content")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Message("Conversion returned no document".into()))?;
    if args.output.out.is_some() {
        deliver_bytes(&args.output, content.as_bytes(), &inputs.protected)?;
    }
    if args.json {
        stdout(&report)?;
    } else if args.output.out.is_none() {
        stdout_bytes(content.as_bytes())?;
    }
    summary(&report);
    status_code(&report)
}

fn review(args: Review, timeout_ms: u64) -> Result<i32> {
    let saved: ConversionReport = serde_json::from_value(files::json(&args.report)?)?;
    // Check the entire saved artifact before trusting its original inputs or IDs.
    call(
        json!({"action":"export","report":saved,"format":"manifest"}),
        timeout_ms,
    )?;
    let mut request: ConversionRequest = saved.original;
    let mut protected = BTreeSet::from([args.report.canonicalize()?]);
    if let Some(path) = args.config {
        request.config = files::config(&path)?;
        protected.insert(path.canonicalize()?);
    }
    if !args.accept.is_empty() {
        // Configuration changes can alter stable decision IDs. Generate the
        // current review set before checking any acknowledgements.
        let pending = call(json!({"action":"review","request":request}), timeout_ms)?;
        let current: ConversionReport = serde_json::from_value(pending)?;
        for id in args.accept {
            let decision = current
                .decisions
                .iter()
                .find(|decision| decision.id == id)
                .ok_or_else(|| Error::Message(format!("Unknown current review decision: {id}")))?;
            if !decision.review_required {
                return Err(Error::Message(format!(
                    "Decision cannot be acknowledged: {}",
                    decision.code
                )));
            }
            request.config.accepted_decisions.insert(id);
        }
    }
    let report = call(json!({"action":"review","request":request}), timeout_ms)?;
    deliver(&args.output, &report, &protected)?;
    summary(&report);
    status_code(&report)
}

fn batch(args: Batch, timeout_ms: u64) -> Result<i32> {
    let mut inputs = files::Inputs::new(&args.settings)?;
    let roots = inputs.expand(&args.inputs, true)?;
    if roots.is_empty() {
        return Err(Error::Message(
            "No supported source filenames selected".into(),
        ));
    }
    if args.out_dir.exists() {
        return Err(Error::Message("Batch out-dir must be new".into()));
    }
    let parent = args
        .out_dir
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = tempfile::tempdir_in(parent)?;
    let mut entries = Vec::new();
    let mut code = 0;
    for root in roots {
        let relative = root
            .strip_prefix(&inputs.root)
            .map_err(|_| Error::Message("Batch source outside root".into()))?;
        let name = stable_name(relative);
        let result = inputs
            .request(&root)
            .and_then(|request| call(json!({"action":"convert","request":request}), timeout_ms));
        match result {
            Ok(report) => {
                code = code.max(status_code(&report)?);
                let report_name = format!("{name}.report.json");
                files::write(
                    &temporary.path().join(&report_name),
                    &serde_json::to_vec_pretty(&report)?,
                    false,
                    &inputs.protected,
                )?;
                let mut entry = json!({"source":relative,"status":report["status"],"valid":report["valid"],"report":report_name,"output":null,"sha256":report.pointer("/output/sha256")});
                if let Some(content) = report.pointer("/output/content").and_then(Value::as_str) {
                    let extension = if report.pointer("/output/encoding").and_then(Value::as_str)
                        == Some("yaml")
                    {
                        "yaml"
                    } else {
                        "json"
                    };
                    let output_name = format!("{name}.openapi.{extension}");
                    files::write(
                        &temporary.path().join(&output_name),
                        content.as_bytes(),
                        false,
                        &inputs.protected,
                    )?;
                    entry["output"] = json!(output_name);
                }
                entries.push(entry);
            }
            Err(error) => {
                code = code.max(if matches!(error, Error::Timeout) {
                    3
                } else {
                    2
                });
                entries.push(json!({"source":relative,"status":"error","error":error.to_string()}));
            }
        }
    }
    let aggregate = json!({"schemaVersion":"1.0.0","engineVersion":octri_converter::ENGINE_VERSION,"results":entries});
    files::write(
        &temporary.path().join("aggregate.json"),
        &serde_json::to_vec_pretty(&aggregate)?,
        false,
        &inputs.protected,
    )?;
    publish_directory(temporary, &args.out_dir)?;
    stdout(&aggregate)?;
    Ok(code)
}

fn stable_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("source");
    let stem: String = stem
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    let normalized = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let hash = format!("{:x}", Sha256::digest(normalized.as_bytes()));
    format!("{stem}-{}", &hash[..16])
}

fn publish_artifact(destination: &Path, artifact: &Value) -> Result<()> {
    if destination.exists() {
        return Err(Error::Message("Artifact destination must be new".into()));
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = tempfile::tempdir_in(parent)?;
    let entries = artifact["files"]
        .as_array()
        .ok_or_else(|| Error::Message("Artifact has no files".into()))?;
    let mut names = BTreeSet::new();
    for entry in entries {
        let name = entry["path"]
            .as_str()
            .ok_or_else(|| Error::Message("Artifact file has no path".into()))?;
        let path = Path::new(name);
        if path.as_os_str().is_empty()
            || !path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            || name.contains('\\')
            || !names.insert(name)
        {
            return Err(Error::Message(
                "Artifact contains an unsafe or repeated path".into(),
            ));
        }
        let content = entry["content"]
            .as_str()
            .ok_or_else(|| Error::Message("Artifact file has no content".into()))?;
        let path = temporary.path().join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        files::write(&path, content.as_bytes(), false, &BTreeSet::new())?;
    }
    publish_directory(temporary, destination)
}

fn publish_directory(temporary: tempfile::TempDir, destination: &Path) -> Result<()> {
    // Reserve the name without replacing any existing directory. Moving each
    // file into that reservation keeps no-clobber semantics on every platform.
    fs::create_dir(destination)?;
    let result = fs::read_dir(temporary.path())?.try_for_each(|entry| {
        let entry = entry?;
        fs::rename(entry.path(), destination.join(entry.file_name()))
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    result?;
    Ok(())
}

fn output_identity(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(parent.canonicalize()?.join(
        path.file_name()
            .ok_or_else(|| Error::Message("Output must name a file".into()))?,
    ))
}

fn status_code(report: &Value) -> Result<i32> {
    match report["status"].as_str() {
        Some("complete") => Ok(0),
        Some("needs-review" | "blocked") => Ok(1),
        _ => Err(Error::Message("Unknown conversion status".into())),
    }
}

fn summary(report: &Value) {
    let status = report["status"].as_str().unwrap_or("unknown");
    let pending = report["decisions"].as_array().map_or(0, |decisions| {
        decisions
            .iter()
            .filter(|decision| decision["reviewRequired"] == true && decision["reviewed"] != true)
            .count()
    });
    eprintln!("Conversion: {status}; {pending} decisions require review.");
}

fn deliver(output: &Destination, value: &Value, protected: &BTreeSet<PathBuf>) -> Result<()> {
    deliver_bytes(output, &serde_json::to_vec_pretty(value)?, protected)
}

fn deliver_bytes(
    output: &Destination,
    content: &[u8],
    protected: &BTreeSet<PathBuf>,
) -> Result<()> {
    if let Some(path) = &output.out {
        files::write(path, content, output.force, protected)
    } else {
        stdout_bytes(content)
    }
}

fn stdout(value: &Value) -> Result<()> {
    stdout_bytes(&serde_json::to_vec_pretty(value)?)
}

fn stdout_bytes(content: &[u8]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(content)?;
    if !content.ends_with(b"\n") {
        stdout.write_all(b"\n")?;
    }
    Ok(())
}
