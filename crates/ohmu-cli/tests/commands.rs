use std::process::{Command, Output};

use serde_json::Value;
use tempfile::tempdir;

fn ohmu(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ohmu"))
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn init_doctor_and_read_work_without_credentials() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let root = root.to_str().unwrap();
    for arguments in [
        vec!["--json", "init", root, "--name", "研究"],
        vec!["--json", "doctor", "--wiki", root],
        vec!["--json", "read", "welcome", "--wiki", root],
    ] {
        let output = ohmu(&arguments);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["api_version"], 1);
        assert!(value["result"].is_object());
        assert!(output.stderr.is_empty());
    }
    let output = ohmu(&["read", "welcome", "--wiki", root]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("sha256:"));
    assert!(!ohmu(&["init", root]).status.success());
}

#[test]
fn runtime_errors_are_json_on_stderr_and_planned_commands_are_not_exposed() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("missing");
    let output = ohmu(&["--json", "doctor", "--wiki", root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let value: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["error"]["code"], "operation_failed");
    assert_eq!(ohmu(&["search", "anything"]).status.code(), Some(2));
}

#[test]
fn markdown_ingest_list_paginated_read_and_catalog_rebuild_form_a_complete_cli_flow() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let input = temp.path().join("paper.md");
    std::fs::write(&input, "# 研究\nPersistent evidence.\n").unwrap();
    let wiki = root.to_str().unwrap();
    assert!(ohmu(&["init", wiki]).status.success());
    let output = ohmu(&["ingest", input.to_str().unwrap(), "--wiki", wiki, "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let id = report["result"]["record"]["source_id"].as_str().unwrap();
    let revision = report["result"]["record"]["revision"].as_str().unwrap();
    let output = ohmu(&[
        "source",
        id,
        "--revision",
        revision,
        "--limit",
        "12",
        "--wiki",
        wiki,
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let part: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        part["result"]["content"]
            .as_str()
            .unwrap()
            .starts_with("# 研究")
    );
    assert!(part["result"]["next_offset"].is_number());
    std::fs::remove_file(root.join(".ohmu/catalog.sqlite")).unwrap();
    assert!(ohmu(&["catalog-rebuild", "--wiki", wiki]).status.success());
    let output = ohmu(&["sources", "--wiki", wiki, "--json"]);
    let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(listed["result"]["sources"][0]["source_id"], id);
}

#[test]
fn html_cli_ingestion_exposes_parser_diagnostics_and_normalized_passages() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let input = temp.path().join("article.html");
    let paragraph =
        "HTMLRESEARCH provides source evidence with sufficient context and detailed findings. "
            .repeat(30);
    std::fs::write(&input, format!("<!doctype html><html><head><title>HTML study</title></head><body><article><h1>HTML study</h1><p>{paragraph}</p></article></body></html>")).unwrap();
    let wiki = root.to_str().unwrap();
    assert!(ohmu(&["init", wiki]).status.success());
    let output = ohmu(&["ingest", input.to_str().unwrap(), "--wiki", wiki, "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["result"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic
                .as_str()
                .unwrap()
                .contains("normalized to Markdown"))
    );
    let id = report["result"]["record"]["source_id"].as_str().unwrap();
    let revision = report["result"]["record"]["revision"].as_str().unwrap();
    let output = ohmu(&[
        "source",
        id,
        "--revision",
        revision,
        "--wiki",
        wiki,
        "--json",
    ]);
    assert!(output.status.success());
    let passage: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        passage["result"]["content"]
            .as_str()
            .unwrap()
            .contains("HTMLRESEARCH")
    );
    assert!(passage["result"]["locator"]["page"].is_null());
}
