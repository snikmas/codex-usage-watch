use std::fs;
use std::process::Command;

use serde_json::Value;

fn command(state: &tempfile::TempDir, codex_home: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codex-5h"));
    command
        .env("CODEX_USAGE_WATCH_HOME", state.path())
        .env("CODEX_HOME", codex_home.path());
    command
}

#[test]
fn help_version_and_invalid_arguments_have_documented_exit_behavior() {
    for flag in ["--help", "-h", "--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_codex-5h"))
            .arg(flag)
            .output()
            .unwrap();
        assert!(output.status.success(), "{flag}");
        assert!(!output.stdout.is_empty());
    }
    let invalid = Command::new(env!("CARGO_BIN_EXE_codex-5h"))
        .arg("not-a-command")
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("--help"));
}

#[test]
fn status_json_is_versioned_and_reports_supported_threshold_configuration() {
    let state = tempfile::tempdir().unwrap();
    let codex_home = tempfile::tempdir().unwrap();
    let output = command(&state, &codex_home)
        .args(["status", "--json"])
        .env("CODEX_USAGE_WATCH_THRESHOLDS", "60, 80,100,80")
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["contract"], "codex-usage-watch.status.v1");
    assert_eq!(value["display"]["schema_version"], 1);
    assert_eq!(
        value["warning_thresholds_percent"],
        serde_json::json!([60, 80, 100])
    );

    let invalid = command(&state, &codex_home)
        .args(["status", "--json"])
        .env("CODEX_USAGE_WATCH_THRESHOLDS", "80,nope")
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
}

#[test]
fn refresh_is_explicit_and_bounded_and_doctor_validates_real_hooks() {
    let state = tempfile::tempdir().unwrap();
    let codex_home = tempfile::tempdir().unwrap();
    let transcript = state.path().join("empty.jsonl");
    fs::write(&transcript, b"").unwrap();

    let refresh = command(&state, &codex_home)
        .args(["refresh", "--transcript", transcript.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(refresh.status.success());
    assert!(String::from_utf8_lossy(&refresh.stdout).contains("1 transcript(s), maximum 8"));

    let missing = command(&state, &codex_home).arg("doctor").output().unwrap();
    assert!(!missing.status.success());

    assert!(
        command(&state, &codex_home)
            .args(["install", "--confirm"])
            .status()
            .unwrap()
            .success()
    );
    let healthy = command(&state, &codex_home).arg("doctor").output().unwrap();
    assert!(healthy.status.success());
    assert!(String::from_utf8_lossy(&healthy.stdout).contains("installed and valid"));

    let mut hooks: Value =
        serde_json::from_slice(&fs::read(codex_home.path().join("hooks.json")).unwrap()).unwrap();
    hooks["hooks"]["Stop"][0]["hooks"][0]["command"] =
        Value::String("missing-program hook stop".into());
    fs::write(
        codex_home.path().join("hooks.json"),
        serde_json::to_vec(&hooks).unwrap(),
    )
    .unwrap();
    let broken = command(&state, &codex_home).arg("doctor").output().unwrap();
    assert!(!broken.status.success());
}
