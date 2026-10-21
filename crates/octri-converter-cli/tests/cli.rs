//! Exercise the installed binary contract, including host I/O and worker exit codes.
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

type Result = std::result::Result<(), Box<dyn std::error::Error>>;

fn command(root: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_octri-spec-convert"))
        .current_dir(root)
        .args(args)
        .output()
}

fn source(title: &str) -> String {
    json!({"openapi":"3.1.2","info":{"title":title,"version":"1"},"paths":{"/pets":{"get":{"responses":{"200":{"description":"Pets"}}}}}}).to_string()
}

#[test]
fn stdin_abi_and_human_help_remain_distinct() -> Result {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_octri-spec-convert"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("missing stdin")?
        .write_all(br#"{"action":"formats","abiVersion":1}"#)?;
    let result = child.wait_with_output()?;
    assert!(result.status.success());
    let result: Value = serde_json::from_slice(&result.stdout)?;
    assert!(
        result["Ok"]["targets"]
            .as_array()
            .is_some_and(|versions| versions.contains(&json!("3.2.1")))
    );
    let temp = tempfile::tempdir()?;
    let help = command(temp.path(), &["convert", "--help"])?;
    assert!(
        help.status.success(),
        "{}",
        String::from_utf8_lossy(&help.stderr)
    );
    assert!(String::from_utf8(help.stdout)?.contains("--root-dir"));
    assert_eq!(
        command(temp.path(), &["formats", "--timeout-ms", "30001"])?
            .status
            .code(),
        Some(2)
    );
    Ok(())
}

#[test]
fn explicit_dependencies_bundle_and_atomic_no_clobber_preserve_sources() -> Result {
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("root.json"), json!({"openapi":"3.1.2","info":{"title":"Pets","version":"1"},"paths":{},"components":{"schemas":{"Pet":{"$ref":"model.json"}}}}).to_string())?;
    fs::write(
        temp.path().join("model.json"),
        r#"{"type":"object","properties":{"id":{"type":"integer"}}}"#,
    )?;
    let args = [
        "bundle",
        "root.json",
        "--file",
        "model.json",
        "--out",
        "result.json",
        "--report",
        "report.json",
    ];
    let result = command(temp.path(), &args)?;
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value =
        serde_json::from_str(&fs::read_to_string(temp.path().join("report.json"))?)?;
    assert_eq!(report["manifest"]["root"], "memory://project/root.json");
    assert_eq!(
        report["manifest"]["files"].as_array().map(Vec::len),
        Some(2)
    );
    // A relocated bundled output must validate without the original files.
    assert_eq!(
        command(temp.path(), &["convert", "result.json", "--json"])?
            .status
            .code(),
        Some(0)
    );
    assert_eq!(command(temp.path(), &args)?.status.code(), Some(2));
    let original = fs::read(temp.path().join("root.json"))?;
    let refused = command(
        temp.path(),
        &[
            "convert",
            "root.json",
            "--file",
            "model.json",
            "--out",
            "root.json",
            "--force",
        ],
    )?;
    assert_eq!(refused.status.code(), Some(2));
    assert_eq!(fs::read(temp.path().join("root.json"))?, original);
    let conflict = command(
        temp.path(),
        &[
            "convert",
            "root.json",
            "--file",
            "model.json",
            "--out",
            "same.json",
            "--report",
            "same.json",
            "--force",
        ],
    )?;
    assert_eq!(conflict.status.code(), Some(2));
    assert!(!temp.path().join("same.json").exists());
    Ok(())
}

#[test]
fn batch_keeps_per_file_errors_stable_names_and_isolated_sources() -> Result {
    let temp = tempfile::tempdir()?;
    fs::create_dir_all(temp.path().join("inputs/one"))?;
    fs::create_dir_all(temp.path().join("inputs/two"))?;
    fs::write(temp.path().join("inputs/one/api.json"), source("First"))?;
    fs::write(temp.path().join("inputs/two/api.json"), source("Second"))?;
    fs::write(temp.path().join("inputs/broken.json"), "invalid")?;
    let result = command(
        temp.path(),
        &["batch", "inputs/**/*.json", "--out-dir", "results"],
    )?;
    assert_eq!(
        result.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let aggregate: Value = serde_json::from_slice(&result.stdout)?;
    let entries = aggregate["results"]
        .as_array()
        .ok_or("missing batch results")?;
    assert_eq!(entries.len(), 3);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["status"] == "error")
            .count(),
        1
    );
    let mut names = std::collections::BTreeSet::new();
    for entry in entries.iter().filter(|entry| entry["status"] != "error") {
        let name = entry["report"].as_str().ok_or("missing report filename")?;
        names.insert(name);
        let report: Value =
            serde_json::from_str(&fs::read_to_string(temp.path().join("results").join(name))?)?;
        assert_eq!(
            report["manifest"]["files"].as_array().map(Vec::len),
            Some(1)
        );
    }
    assert_eq!(names.len(), 2);
    let again = command(temp.path(), &["batch", "inputs", "--out-dir", "again"])?;
    assert_eq!(again.stdout, result.stdout);
    assert_eq!(
        command(temp.path(), &["batch", "inputs", "--out-dir", "results"])?
            .status
            .code(),
        Some(2)
    );
    Ok(())
}

#[test]
fn reviewed_reports_recompute_and_exports_refuse_tampering() -> Result {
    let temp = tempfile::tempdir()?;
    fs::write(
        temp.path().join("request.curl"),
        "curl https://api.example.com/pets",
    )?;
    let initial = command(temp.path(), &["convert", "request.curl", "--json"])?;
    assert_eq!(
        initial.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let report: Value = serde_json::from_slice(&initial.stdout)?;
    fs::write(temp.path().join("initial.json"), &initial.stdout)?;
    let mut args = vec!["review", "initial.json", "--out", "reviewed.json"];
    let ids: Vec<&str> = report["decisions"]
        .as_array()
        .ok_or("missing decisions")?
        .iter()
        .filter(|decision| decision["reviewRequired"] == true)
        .filter_map(|decision| decision["id"].as_str())
        .collect();
    assert!(!ids.is_empty());
    for id in ids {
        args.extend(["--accept", id]);
    }
    let reviewed = command(temp.path(), &args)?;
    assert_eq!(
        reviewed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&reviewed.stderr)
    );
    assert_eq!(
        command(
            temp.path(),
            &["review", "initial.json", "--accept", "unknown"]
        )?
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        command(
            temp.path(),
            &[
                "export",
                "reviewed.json",
                "--format",
                "artifact",
                "--out",
                "artifact"
            ]
        )?
        .status
        .code(),
        Some(0)
    );
    assert!(temp.path().join("artifact/report.json").exists());
    let compared = command(temp.path(), &["compare", "initial.json", "reviewed.json"])?;
    assert_eq!(compared.status.code(), Some(0));
    let compared: Value = serde_json::from_slice(&compared.stdout)?;
    assert_eq!(compared["changes"], json!([]));
    let mut tampered = report;
    tampered["status"] = json!("complete");
    fs::write(temp.path().join("tampered.json"), tampered.to_string())?;
    assert_eq!(
        command(
            temp.path(),
            &["export", "tampered.json", "--format", "html"]
        )?
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        command(temp.path(), &["review", "tampered.json"])?
            .status
            .code(),
        Some(2)
    );
    Ok(())
}

#[test]
fn bounded_reader_and_deadline_return_distinct_failures() -> Result {
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("api.json"), source("Pets"))?;
    fs::write(
        temp.path().join("small.json"),
        r#"{"limits":{"maxDocumentBytes":10}}"#,
    )?;
    assert_eq!(
        command(
            temp.path(),
            &["convert", "api.json", "--config", "small.json"]
        )?
        .status
        .code(),
        Some(2)
    );
    let paths: serde_json::Map<String, Value> = (0..2000)
        .map(|index| {
            (
                format!("/pets/{index}"),
                json!({"get":{"responses":{"200":{"description":"Pets"}}}}),
            )
        })
        .collect();
    fs::write(
        temp.path().join("large.json"),
        json!({"openapi":"3.1.2","info":{"title":"Large fixture","version":"1"},"paths":paths})
            .to_string(),
    )?;
    let result = command(temp.path(), &["convert", "large.json", "--timeout-ms", "1"])?;
    assert_eq!(
        result.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!temp.path().join("result.json").exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn root_boundary_rejects_symlink_escape_and_parent_traversal() -> Result {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("project");
    fs::create_dir(&root)?;
    fs::write(temp.path().join("outside.json"), source("Outside"))?;
    symlink(temp.path().join("outside.json"), root.join("linked.json"))?;
    for name in ["linked.json", "../outside.json"] {
        let result = command(&root, &["convert", name])?;
        assert_eq!(result.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&result.stderr).contains("escapes root-dir"));
    }
    Ok(())
}
