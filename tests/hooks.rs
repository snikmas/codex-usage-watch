use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
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
        .execute_batch("BEGIN EXCLUSIVE")
        .expect("hold writer lock");
    let started = Instant::now();
    let output = run_hook(
        &state,
        "user-prompt-submit",
        json!({"hook_event_name": "UserPromptSubmit", "transcript_path": null}),
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).expect("timeout fail-open JSON"),
        json!({"continue": true, "suppressOutput": true})
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("database is locked"));
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
            "hooks": {
                "SessionStart": [{"matcher": "shared", "hooks": [
                    {"type": "command", "command": "other"},
                    {"type": "command", "command": "codex-5h hook session-start"}
                ]}],
                "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "other-prompt"}]}]
            }
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

    for args in [["install", "--confirm"], ["install", "--confirm"]] {
        assert!(
            Command::new(env!("CARGO_BIN_EXE_codex-5h"))
                .args(args)
                .env("CODEX_HOME", home.path())
                .status()
                .expect("run hook configuration command")
                .success()
        );
    }
    let installed: Value = serde_json::from_slice(
        &fs::read(home.path().join("hooks.json")).expect("read installed hooks"),
    )
    .expect("parse installed hooks");
    let encoded = installed.to_string();
    assert_eq!(encoded.matches("session-start").count(), 1);
    assert_eq!(encoded.matches("user-prompt-submit").count(), 1);
    assert_eq!(encoded.matches(" hook stop").count(), 1);
    for (event, command_name) in [
        ("SessionStart", "session-start"),
        ("UserPromptSubmit", "user-prompt-submit"),
        ("Stop", "stop"),
    ] {
        let handlers = installed["hooks"][event]
            .as_array()
            .expect("event groups")
            .iter()
            .flat_map(|group| group["hooks"].as_array().expect("handlers"));
        let matching: Vec<_> = handlers
            .filter(|handler| {
                handler["command"]
                    .as_str()
                    .is_some_and(|command| command.ends_with(&format!(" hook {command_name}")))
            })
            .collect();
        assert_eq!(matching.len(), 1, "exactly one installed {event} handler");
        assert_eq!(matching[0]["type"], "command");
        assert_eq!(matching[0]["timeout"], 5);
        let executable = command_executable(
            matching[0]["command"]
                .as_str()
                .expect("installed command string"),
            command_name,
        );
        assert_paths_equivalent(&executable, Path::new(env!("CARGO_BIN_EXE_codex-5h")));
    }

    for _ in 0..2 {
        assert!(
            Command::new(env!("CARGO_BIN_EXE_codex-5h"))
                .args(["uninstall", "--confirm"])
                .env("CODEX_HOME", home.path())
                .status()
                .expect("run repeated uninstall")
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
    assert_eq!(hooks["hooks"]["SessionStart"][0]["matcher"], "shared");
    assert!(hooks.to_string().contains("other-prompt"));
    assert!(!hooks.to_string().contains("codex-5h"));
    let backup: Value = serde_json::from_slice(
        &fs::read(home.path().join("hooks.json.codex-5h.bak")).expect("read backup"),
    )
    .expect("backup remains recoverable JSON");
    assert!(backup.to_string().contains("other"));
}

fn command_executable(command: &str, command_name: &str) -> PathBuf {
    let suffix = format!(" hook {command_name}");
    let encoded = command
        .strip_suffix(&suffix)
        .expect("command has the correct hook event")
        .trim();
    #[cfg(unix)]
    if let Some(body) = encoded
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
    {
        return PathBuf::from(body.replace("'\"'\"'", "'"));
    }
    PathBuf::from(encoded.trim_matches('"').replace("\\\"", "\""))
}

fn assert_paths_equivalent(actual: &Path, expected: &Path) {
    let actual = fs::canonicalize(actual).expect("installed executable resolves");
    let expected = fs::canonicalize(expected).expect("Cargo executable resolves");
    if cfg!(windows) {
        assert!(
            actual
                .to_string_lossy()
                .eq_ignore_ascii_case(&expected.to_string_lossy()),
            "Windows executable paths differ: {} != {}",
            actual.display(),
            expected.display()
        );
    } else {
        assert_eq!(actual, expected);
    }
}

#[cfg(unix)]
fn publish_test_binary(destination: &Path) {
    let staged = destination.with_file_name(".codex-5h.tmp");
    fs::copy(env!("CARGO_BIN_EXE_codex-5h"), &staged).expect("stage tracker binary");
    fs::File::open(&staged)
        .expect("open staged tracker binary")
        .sync_all()
        .expect("sync staged tracker binary");
    fs::rename(&staged, destination).expect("publish tracker binary atomically");
    // The sandbox uses a bind-mounted filesystem where an immediately executed
    // copy can briefly retain ETXTBSY after its writer closes.
    std::thread::sleep(Duration::from_millis(20));
}

#[test]
fn malformed_hooks_and_interrupted_temp_files_never_replace_user_configuration() {
    let home = tempfile::tempdir().expect("temporary Codex home");
    let hooks_path = home.path().join("hooks.json");
    fs::write(&hooks_path, b"{not valid json").expect("write malformed hooks");
    fs::write(home.path().join(".hooks.json.interrupted"), b"partial").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_codex-5h"))
        .args(["install", "--confirm"])
        .env("CODEX_HOME", home.path())
        .output()
        .expect("run install against malformed hooks");
    assert!(!output.status.success());
    assert_eq!(fs::read(&hooks_path).unwrap(), b"{not valid json");
    assert!(!home.path().join("hooks.json.codex-5h.bak").exists());
}

#[cfg(unix)]
#[test]
fn generated_hook_commands_execute_adversarial_paths_literally() {
    let temp = tempfile::tempdir().expect("temporary adversarial path root");
    let component = "bin space 'single' \"double\" $dollar `touch injected-backtick` $(touch injected-substitution) \\backslash";
    let bin_dir = temp.path().join(component);
    fs::create_dir_all(&bin_dir).expect("create adversarial binary directory");
    let copied_binary = bin_dir.join("codex-5h");
    publish_test_binary(&copied_binary);

    let codex_home = temp.path().join("codex home");
    let state = temp.path().join("state");
    let install = Command::new(&copied_binary)
        .args(["install", "--confirm"])
        .env("CODEX_HOME", &codex_home)
        .env("CODEX_USAGE_WATCH_HOME", &state)
        .output()
        .expect("install adversarial hook path");
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stderr)
    );

    let hooks: Value = serde_json::from_slice(
        &fs::read(codex_home.join("hooks.json")).expect("read adversarial hooks"),
    )
    .expect("parse adversarial hooks");
    for (wire_event, command_event) in [
        ("SessionStart", "session-start"),
        ("UserPromptSubmit", "user-prompt-submit"),
        ("Stop", "stop"),
    ] {
        let command = hooks["hooks"][wire_event][0]["hooks"][0]["command"]
            .as_str()
            .expect("installed command");
        assert_eq!(command_executable(command, command_event), copied_binary);

        let mut child = Command::new("/bin/sh")
            .args(["-c", command])
            .current_dir(temp.path())
            .env("CODEX_HOME", &codex_home)
            .env("CODEX_USAGE_WATCH_HOME", &state)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("execute generated hook through POSIX shell");
        child
            .stdin
            .take()
            .expect("hook stdin")
            .write_all(
                json!({"hook_event_name": wire_event, "transcript_path": null})
                    .to_string()
                    .as_bytes(),
            )
            .expect("write hook JSON");
        let output = child.wait_with_output().expect("wait for adversarial hook");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout)
                .expect("generated hook stdout remains JSON")["continue"],
            true
        );
    }

    assert!(!temp.path().join("injected-backtick").exists());
    assert!(!temp.path().join("injected-substitution").exists());
}

// Linux filesystems can represent this byte sequence. macOS rejects it before
// the installer can inspect the executable path, so there is no product-level
// rejection path to exercise there.
#[cfg(target_os = "linux")]
#[test]
fn hook_install_rejects_a_non_utf8_executable_path_without_writing_hooks() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let temp = tempfile::tempdir().expect("temporary non-UTF-8 path root");
    let component = OsString::from_vec(b"invalid-utf8-\xff".to_vec());
    let bin_dir = temp.path().join(component);
    fs::create_dir_all(&bin_dir).expect("create non-UTF-8 binary directory");
    let copied_binary = bin_dir.join("codex-5h");
    publish_test_binary(&copied_binary);
    let codex_home = temp.path().join("codex-home");

    let output = Command::new(&copied_binary)
        .args(["install", "--confirm"])
        .env("CODEX_HOME", &codex_home)
        .output()
        .expect("attempt non-UTF-8 hook install");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cannot be represented safely"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!codex_home.join("hooks.json").exists());
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
