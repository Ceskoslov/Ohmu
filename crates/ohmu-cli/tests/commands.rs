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
