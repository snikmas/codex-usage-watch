use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use rusqlite::Connection;
use tempfile::TempDir;

fn history_fixture(home: &TempDir) -> std::path::PathBuf {
    let sessions = home.path().join("sessions/2025/01/01");
    fs::create_dir_all(&sessions).unwrap();
    let transcript = sessions.join("session.jsonl");
    let content = concat!(
        "{\"type\":\"session_meta\",\"payload\":{\"cli_version\":\"0.140.0\",\"model\":\"gpt-old\",\"service_tier\":\"standard\",\"plan_type\":\"plus\",\"instructions\":\"PRIVATE INSTRUCTIONS\"}}\n",
        "{\"timestamp\":\"2025-01-01T00:00:00Z\",\"type\":\"response_item\",\"payload\":{\"prompt\":\"PRIVATE PROMPT\",\"source_code\":\"SECRET\"}}\n",
        "{\"timestamp\":\"2025-01-01T00:01:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"rate_limits\":{\"plan_type\":\"plus\",\"primary\":{\"used_percent\":10,\"window_minutes\":10080,\"resets_at\":1736294400}}}}\n"
    );
    fs::write(&transcript, content).unwrap();
    transcript
}

#[test]
fn preview_and_decline_do_not_read_or_create_usage_state() {
    let home = TempDir::new().unwrap();
    history_fixture(&home);
    let state = home.path().join("state");
    let preview = Command::new(env!("CARGO_BIN_EXE_codex-watch"))
        .args(["setup", "--preview"])
        .env("CODEX_HOME", home.path())
        .env("CODEX_USAGE_WATCH_HOME", &state)
        .output()
        .unwrap();
    assert!(preview.status.success());
    let stdout = String::from_utf8(preview.stdout).unwrap();
    assert!(stdout.contains("Candidate files      1"));
    assert!(stdout.contains("prompts, responses, tool arguments"));
    assert!(!stdout.contains("PRIVATE PROMPT"));
    assert!(!state.exists());

    let mut child = Command::new(env!("CARGO_BIN_EXE_codex-watch"))
        .arg("setup")
        .env("CODEX_HOME", home.path())
        .env("CODEX_USAGE_WATCH_HOME", &state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"n\n").unwrap();
    let declined = child.wait_with_output().unwrap();
    assert!(declined.status.success());
    assert!(
        String::from_utf8(declined.stdout)
            .unwrap()
            .contains("no transcript contents were read")
    );
    assert!(!state.exists());
}

#[test]
fn consented_weekly_only_import_retains_only_structured_metadata() {
    let home = TempDir::new().unwrap();
    history_fixture(&home);
    let state = home.path().join("state");
    let output = Command::new(env!("CARGO_BIN_EXE_codex-watch"))
        .args(["setup", "--import", "--confirm"])
        .env("CODEX_HOME", home.path())
        .env("CODEX_USAGE_WATCH_HOME", &state)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Weekly-only evidence 1 observation(s)"));
    assert!(stdout.contains("Personal calibration not identifiable"));
    assert!(!stdout.contains("PRIVATE PROMPT"));

    let database = state.join("state.sqlite3");
    let connection = Connection::open(database).unwrap();
    let observations: i64 = connection
        .query_row("SELECT COUNT(*) FROM rate_limit_observations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(observations, 1);
    let (version, generation): (String, String) = connection
        .query_row(
            "SELECT codex_version, compatibility_generation FROM rate_limit_observations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(version, "0.140.0");
    assert_eq!(generation, "codex-version:0.140.0");
    let schema: String = connection
        .query_row(
            "SELECT group_concat(name, ',') FROM pragma_table_info('rate_limit_observations')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!schema.contains("prompt"));
    assert!(!schema.contains("response"));
    assert!(!schema.contains("source_code"));
}
