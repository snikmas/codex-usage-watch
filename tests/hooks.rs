use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{TimeDelta, Utc};
use rusqlite::Connection;
use serde_json::{Value, json};

fn run_hook(state: &tempfile::TempDir, event: &str, input: Value) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_codex-5h"))
        .args(["hook", event])
        .env("CODEX_USAGE_WATCH_HOME", state.path())
        .env("CODEX_HOME", state.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start hook adapter");
    child
        .stdin
        .take()
        .expect("hook stdin")
        .write_all(
            serde_json::to_string(&input)
                .expect("serialize input")
                .as_bytes(),
        )
        .expect("write hook input");
    child.wait_with_output().expect("wait for hook adapter")
}

fn write_transcript(path: &std::path::Path, age: TimeDelta, final_used: f64) {
    let observed = Utc::now() - age;
    let reset = (observed + TimeDelta::days(7)).timestamp();
    let lines = [
        json!({
            "timestamp": (observed - TimeDelta::minutes(1)).to_rfc3339(),
            "type": "event_msg",
            "payload": {"type": "token_count", "rate_limits": {"plan_type": "plus", "primary": {
                "used_percent": 20.0, "window_minutes": 10080, "resets_at": reset
            }}}
        }),
        json!({
            "timestamp": observed.to_rfc3339(),
            "type": "event_msg",
            "payload": {"type": "token_count", "rate_limits": {"plan_type": "plus", "primary": {
                "used_percent": final_used, "window_minutes": 10080, "resets_at": reset
            }}}
        }),
    ];
    fs::write(
        path,
        lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("write transcript");
}

fn hook_input(event: &str, transcript: &std::path::Path) -> Value {
    json!({"hook_event_name": event, "transcript_path": transcript})
}

#[test]
fn normal_warning_super_stale_and_stop_behaviors_are_non_blocking() {
    let normal_state = tempfile::tempdir().expect("normal state");
    let normal = normal_state.path().join("normal.jsonl");
    write_transcript(&normal, TimeDelta::minutes(1), 22.0);
    let output = run_hook(
        &normal_state,
        "session-start",
        hook_input("SessionStart", &normal),
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("normal JSON");
    assert_eq!(parsed["continue"], true);
    assert!(parsed["systemMessage"].as_str().is_some_and(|message| {
        message.contains("estimated") && message.contains("continuing normally")
    }));

    let super_state = tempfile::tempdir().expect("super state");
    let super_transcript = super_state.path().join("super.jsonl");
    write_transcript(&super_transcript, TimeDelta::minutes(1), 38.0);
    let warning = run_hook(
        &super_state,
        "user-prompt-submit",
        hook_input("UserPromptSubmit", &super_transcript),
    );
    let warning: Value = serde_json::from_slice(&warning.stdout).expect("warning JSON");
    assert_eq!(warning["continue"], true);
    assert!(warning["systemMessage"].as_str().is_some_and(|message| {
        message.contains("exceeded") && message.contains("114% estimated")
    }));
    let duplicate = run_hook(
        &super_state,
        "user-prompt-submit",
        hook_input("UserPromptSubmit", &super_transcript),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&duplicate.stdout).expect("duplicate JSON"),
        json!({"continue": true, "suppressOutput": true})
    );
    let stopped = run_hook(&super_state, "stop", hook_input("Stop", &super_transcript));
    assert_eq!(
        serde_json::from_slice::<Value>(&stopped.stdout).expect("stop JSON"),
        json!({"continue": true, "suppressOutput": true})
    );

    let stale_state = tempfile::tempdir().expect("stale state");
    let stale = stale_state.path().join("stale.jsonl");
    write_transcript(&stale, TimeDelta::minutes(20), 22.0);
    let output = run_hook(
        &stale_state,
        "session-start",
        hook_input("SessionStart", &stale),
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("stale JSON");
    assert!(
        parsed["systemMessage"]
            .as_str()
            .is_some_and(|message| message.contains("stale"))
    );
}

#[test]
fn database_contention_stays_inside_hook_timeout_and_fails_open() {
    let state = tempfile::tempdir().expect("temporary state");
    let initial = run_hook(
        &state,
        "session-start",
        json!({"hook_event_name": "SessionStart", "transcript_path": null}),
    );
    assert!(initial.status.success());

    let connection = Connection::open(state.path().join("state.sqlite3")).expect("open state");
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .expect("hold writer lock");
    let started = Instant::now();
    let output = run_hook(
        &state,
        "stop",
        json!({"hook_event_name": "Stop", "transcript_path": null}),
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).expect("timeout fail-open JSON"),
        json!({"continue": true, "suppressOutput": true})
    );
}

#[test]
fn simultaneous_sessions_share_state_and_emit_one_threshold_warning() {
    let state = tempfile::tempdir().expect("shared state");
    let transcript = state.path().join("shared.jsonl");
    write_transcript(&transcript, TimeDelta::minutes(1), 38.0);

    let outputs = std::thread::scope(|scope| {
        let handles = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    run_hook(
                        &state,
                        "user-prompt-submit",
                        hook_input("UserPromptSubmit", &transcript),
                    )
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("hook process"))
            .collect::<Vec<_>>()
    });

    let responses = outputs
        .iter()
        .map(|output| {
            assert!(output.status.success());
            serde_json::from_slice::<Value>(&output.stdout).expect("hook JSON")
        })
        .collect::<Vec<_>>();
    assert!(
        responses
            .iter()
            .all(|response| response["continue"] == true)
    );
    assert_eq!(
        responses
            .iter()
            .filter(|response| response.get("systemMessage").is_some())
            .count(),
        1
    );
}

#[test]
fn missing_data_and_corrupt_database_fail_open_with_json_only_stdout() {
    let state = tempfile::tempdir().expect("temporary state");
    let output = run_hook(
        &state,
        "session-start",
        json!({"hook_event_name": "SessionStart", "transcript_path": null}),
    );
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid hook JSON");
    assert_eq!(parsed["continue"], true);
    assert!(parsed["systemMessage"].as_str().is_some_and(|message| {
        message.contains("not available yet") && message.contains("continuing normally")
    }));

    fs::write(state.path().join("state.sqlite3"), "not sqlite").expect("corrupt database");
    let output = run_hook(
        &state,
        "user-prompt-submit",
        json!({"hook_event_name": "UserPromptSubmit", "transcript_path": null}),
    );
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).expect("valid fail-open JSON"),
        json!({"continue": true, "suppressOutput": true})
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("diagnostic"));
}

#[test]
fn install_and_uninstall_are_explicit_reversible_and_preserve_other_hooks() {
    let home = tempfile::tempdir().expect("temporary Codex home");
    fs::write(
        home.path().join("hooks.json"),
        serde_json::to_vec_pretty(&json!({
            "hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "other"}]}]}
        }))
        .expect("serialize hooks"),
    )
    .expect("write hooks");

    let denied = Command::new(env!("CARGO_BIN_EXE_codex-5h"))
        .arg("install")
        .env("CODEX_HOME", home.path())
        .output()
        .expect("run install without confirmation");
    assert!(!denied.status.success());

    for args in [["install", "--confirm"], ["uninstall", "--confirm"]] {
        assert!(
            Command::new(env!("CARGO_BIN_EXE_codex-5h"))
                .args(args)
                .env("CODEX_HOME", home.path())
                .status()
                .expect("run hook configuration command")
                .success()
        );
    }
    let hooks: Value = serde_json::from_slice(
        &fs::read(home.path().join("hooks.json")).expect("read final hooks"),
    )
    .expect("parse final hooks");
    assert_eq!(
        hooks["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "other"
    );
    assert!(!hooks.to_string().contains("codex-5h hook"));
}

#[test]
fn shipped_plugin_bounds_all_hook_commands_to_five_seconds() {
    let hooks: Value = serde_json::from_str(include_str!("../hooks/hooks.json"))
        .expect("shipped hooks are valid JSON");
    let handlers = hooks["hooks"]
        .as_object()
        .expect("hook event object")
        .values()
        .flat_map(|groups| groups.as_array().expect("matcher groups"))
        .flat_map(|group| group["hooks"].as_array().expect("handlers"));
    assert!(handlers.into_iter().all(|handler| handler["timeout"] == 5));
}

#[test]
fn session_start_reports_each_compatibility_identity_once() {
    let state = tempfile::tempdir().expect("compatibility state");
    let transcript = state.path().join("compatibility.jsonl");
    write_transcript(&transcript, TimeDelta::minutes(1), 22.0);
    let input = json!({
        "hook_event_name": "SessionStart",
        "transcript_path": transcript,
        "codex_version": "0.146.0",
        "model": "gpt-test",
        "service_tier": "standard"
    });
    let first = run_hook(&state, "session-start", input.clone());
    let repeated = run_hook(&state, "session-start", input);
    let first: Value = serde_json::from_slice(&first.stdout).expect("first hook JSON");
    let repeated: Value = serde_json::from_slice(&repeated.stdout).expect("repeat hook JSON");
    assert_eq!(first["continue"], true);
    assert_eq!(repeated["continue"], true);
    assert!(
        first["systemMessage"]
            .as_str()
            .is_some_and(|message| message.contains("Compatibility:"))
    );
    assert!(
        repeated["systemMessage"]
            .as_str()
            .is_some_and(|message| !message.contains("Compatibility:"))
    );
}
