use std::collections::BTreeSet;
use std::fs;
use std::process::Command;

use serde_json::Value;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn command(state: &tempfile::TempDir, codex_home: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codex-watch"));
    command
        .env("CODEX_USAGE_WATCH_HOME", state.path())
        .env("CODEX_HOME", codex_home.path());
    command
}

#[test]
fn help_version_and_invalid_arguments_have_documented_exit_behavior() {
    for flag in ["--help", "-h", "--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_codex-watch"))
            .arg(flag)
            .output()
            .unwrap();
        assert!(output.status.success(), "{flag}");
        assert!(!output.stdout.is_empty());
    }
    let invalid = Command::new(env!("CARGO_BIN_EXE_codex-watch"))
        .arg("not-a-command")
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid usage"));

    for subcommand in [
        "setup",
        "status",
        "refresh",
        "history",
        "analyze",
        "reset",
        "doctor",
        "calibration",
        "backup",
        "install",
        "uninstall",
        "hook",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_codex-watch"))
            .args([subcommand, "--help"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{subcommand}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("USAGE:"));
    }
    let nested = Command::new(env!("CARGO_BIN_EXE_codex-watch"))
        .args(["calibration", "apply", "--help"])
        .output()
        .unwrap();
    assert!(nested.status.success());
    assert!(String::from_utf8_lossy(&nested.stdout).contains("WEEKLY_POINTS"));
}

#[test]
fn exit_codes_distinguish_usage_configuration_unavailable_and_runtime_failures() {
    let state = tempfile::tempdir().unwrap();
    let codex_home = tempfile::tempdir().unwrap();

    let usage = command(&state, &codex_home)
        .args(["status", "--bogus"])
        .output()
        .unwrap();
    assert_eq!(usage.status.code(), Some(2));
    let unknown_hook = command(&state, &codex_home)
        .args(["hook", "not-an-event"])
        .output()
        .unwrap();
    assert_eq!(unknown_hook.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&unknown_hook.stderr).contains("invalid usage"));

    let configuration = command(&state, &codex_home)
        .arg("status")
        .env("CODEX_USAGE_WATCH_THRESHOLDS", "80,nope")
        .output()
        .unwrap();
    assert_eq!(configuration.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&configuration.stderr).contains("configuration error"));

    let unavailable = command(&state, &codex_home)
        .args(["calibration", "apply", "12.5", "--confirm"])
        .output()
        .unwrap();
    assert_eq!(unavailable.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&unavailable.stderr).contains("unavailable data"));

    fs::write(state.path().join("state.sqlite3"), b"not sqlite").unwrap();
    let runtime = command(&state, &codex_home)
        .arg("history")
        .output()
        .unwrap();
    assert_eq!(runtime.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&runtime.stderr).contains("runtime failure"));
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
    assert!(
        fs::read_dir(state.path()).unwrap().next().is_none(),
        "status must not create or repair state"
    );

    let invalid = command(&state, &codex_home)
        .args(["status", "--json"])
        .env("CODEX_USAGE_WATCH_THRESHOLDS", "80,nope")
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(3));
}

#[test]
fn status_v1_json_field_contract_remains_backward_compatible() {
    let state = tempfile::tempdir().unwrap();
    let codex_home = tempfile::tempdir().unwrap();
    let output = command(&state, &codex_home)
        .args(["status", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let root_keys = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        root_keys,
        BTreeSet::from([
            "active_calibration_weekly_points",
            "contract",
            "display",
            "warning_thresholds_percent",
        ])
    );
    let display_keys = value["display"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        display_keys,
        BTreeSet::from([
            "calibration_confidence",
            "calibration_id",
            "calibration_kind",
            "calibration_weekly_points",
            "data_age_seconds",
            "five_hour_estimate_left_percent",
            "five_hour_estimate_percent",
            "five_hour_value_source",
            "generated_at",
            "observed_at",
            "schema_version",
            "stale",
            "status",
            "weekly_limit_left_percent",
            "weekly_limit_used_percent",
            "weekly_points",
            "window_ends_at",
            "window_started_at",
        ])
    );
    assert_eq!(value["contract"], "codex-usage-watch.status.v1");
    assert_eq!(value["display"]["schema_version"], 1);
    assert_eq!(value["display"]["status"], "unknown");
    assert_eq!(value["display"]["stale"], true);
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
    let missing_stdout = String::from_utf8_lossy(&missing.stdout);
    assert!(missing_stdout.contains("Database schema"));
    assert!(missing_stdout.contains("Display projection"));
    assert!(missing_stdout.contains("Plugin hooks        missing/malformed or path-invalid"));
    assert!(missing_stdout.contains("Session metadata"));
    assert!(missing_stdout.contains("Requests continue   yes"));

    assert!(
        command(&state, &codex_home)
            .args(["install", "--confirm"])
            .status()
            .unwrap()
            .success()
    );
    let healthy = command(&state, &codex_home).arg("doctor").output().unwrap();
    assert!(healthy.status.success());
    let healthy_stdout = String::from_utf8_lossy(&healthy.stdout);
    assert!(healthy_stdout.contains("configured and path-valid"));
    assert!(healthy_stdout.contains("trust must be confirmed inside Codex"));
    assert!(!healthy_stdout.contains("fully trusted"));

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
    let broken_stdout = String::from_utf8_lossy(&broken.stdout);
    assert!(broken_stdout.contains("Database schema"));
    assert!(broken_stdout.contains("Plugin hooks        missing/malformed or path-invalid"));
    assert!(broken_stdout.contains("Requests continue   yes"));
}

#[test]
fn doctor_reports_malformed_hooks_and_corrupt_state_together() {
    let state = tempfile::tempdir().unwrap();
    let codex_home = tempfile::tempdir().unwrap();
    fs::write(state.path().join("state.sqlite3"), b"not sqlite").unwrap();
    fs::write(codex_home.path().join("hooks.json"), b"{not json").unwrap();
    fs::create_dir(codex_home.path().join("sessions")).unwrap();

    let output = command(&state, &codex_home).arg("doctor").output().unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Executable"));
    assert!(stdout.contains("Database schema     unavailable"));
    assert!(stdout.contains("Display projection  unavailable"));
    assert!(stdout.contains("Compatibility      unavailable"));
    assert!(stdout.contains("Plugin hooks        missing/malformed or path-invalid"));
    assert!(stdout.contains("Session metadata    accessible"));
    assert!(stdout.contains("Requests continue   yes"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("independent issue(s)"));
}

#[test]
fn doctor_json_and_support_bundle_are_versioned_and_privacy_sanitized() {
    let state = tempfile::tempdir().unwrap();
    let codex_home = tempfile::tempdir().unwrap();
    let json_output = command(&state, &codex_home)
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(json_output.status.success());
    let report: Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert_eq!(report["contract"], "codex-usage-watch.doctor.v1");
    assert_eq!(report["requests_continue"], true);
    assert_eq!(report["hooks"]["trust"], "must_be_confirmed_inside_codex");

    let encoded = String::from_utf8(json_output.stdout).unwrap();
    for forbidden in [
        state.path().to_string_lossy(),
        codex_home.path().to_string_lossy(),
        std::borrow::Cow::Borrowed("transcript_path"),
        std::borrow::Cow::Borrowed("source_file"),
        std::borrow::Cow::Borrowed("state.sqlite3"),
    ] {
        assert!(!encoded.contains(forbidden.as_ref()));
    }

    let bundle = state.path().join("support.json");
    let bundle_output = command(&state, &codex_home)
        .args([
            "doctor",
            "--support-bundle",
            bundle.to_str().unwrap(),
            "--confirm",
        ])
        .output()
        .unwrap();
    assert!(bundle_output.status.success());
    let bundled: Value = serde_json::from_slice(&fs::read(&bundle).unwrap()).unwrap();
    assert_eq!(bundled["contract"], "codex-usage-watch.doctor.v1");
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(bundle).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
