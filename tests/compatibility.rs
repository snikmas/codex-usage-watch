use std::fs;
use std::io::Write;
use std::process::Command;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

use chrono::{DateTime, TimeDelta, Utc};
use codex_usage_watch::{
    CompatibilityIdentity, CompatibilityResult, IngestOptions, StateStore, TrackerConfig,
    WindowStatus, cached_release_metadata,
};
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn dt(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn valid_line(at: &str, model: &str, tier: &str) -> String {
    format!(
        "{{\"timestamp\":\"{at}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"model\":\"{model}\",\"service_tier\":\"{tier}\",\"rate_limits\":{{\"plan_type\":\"plus\",\"primary\":{{\"used_percent\":20,\"window_minutes\":300,\"resets_at\":1893474000}},\"secondary\":{{\"used_percent\":10,\"window_minutes\":10080,\"resets_at\":1894060800}}}}}}}}"
    )
}

#[test]
fn unseen_identity_checks_once_and_new_model_is_review_not_reset() {
    let temp = TempDir::new().unwrap();
    let transcript = temp.path().join("session.jsonl");
    fs::write(
        &transcript,
        format!(
            "{}\n",
            valid_line("2030-01-01T00:00:00Z", "gpt-old", "standard")
        ),
    )
    .unwrap();
    let mut store =
        StateStore::open_in(temp.path().join("state"), TrackerConfig::default()).unwrap();
    store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-01-01T00:01:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();

    let (old_identity, supported) = store
        .current_compatibility_identity(Some("0.145.0"), None, None)
        .unwrap();
    let first = store
        .check_compatibility(old_identity.clone(), supported, dt("2030-01-01T00:02:00Z"))
        .unwrap();
    let repeated = store
        .check_compatibility(old_identity, supported, dt("2030-01-01T00:03:00Z"))
        .unwrap();
    assert!(first.first_seen);
    assert_eq!(first.result, CompatibilityResult::Compatible);
    assert!(!repeated.first_seen);
    assert_eq!(repeated.checked_at, first.checked_at);

    let (new_identity, supported) = store
        .current_compatibility_identity(Some("0.146.0"), Some("gpt-new"), Some("fast"))
        .unwrap();
    let new_model = store
        .check_compatibility(new_identity, supported, dt("2030-01-01T00:04:00Z"))
        .unwrap();
    assert_eq!(new_model.result, CompatibilityResult::Review);
    assert_eq!(
        new_model.model_confidence,
        "inherited / not validated for this model"
    );
    assert_eq!(store.active_calibration(), 15.8);

    fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .unwrap()
        .write_all(
            format!(
                "{}\n",
                valid_line("2030-01-01T06:00:00Z", "gpt-new", "fast")
            )
            .as_bytes(),
        )
        .unwrap();
    store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-01-01T06:01:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    let inherited = store.selected_calibration_profile().unwrap();
    assert_eq!(inherited.value, Some(15.8));
    assert_eq!(
        inherited.confidence,
        codex_usage_watch::CalibrationConfidence::InheritedUnvalidated
    );
    let connection = rusqlite::Connection::open(store.paths().database.clone()).unwrap();
    let windows: Vec<(String, String)> = connection
        .prepare("SELECT calibration_id, calibration_confidence FROM windows ORDER BY started_at")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(windows.len(), 2);
    assert_ne!(windows[0].0, windows[1].0);
    assert_eq!(windows[1].0, inherited.calibration_id);
    assert_eq!(windows[1].1, "inherited_unvalidated");
}

#[test]
fn simultaneous_first_seen_checks_commit_one_complete_compatibility_transition() {
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("state");
    drop(StateStore::open_in(&state, TrackerConfig::default()).unwrap());
    let identity = CompatibilityIdentity {
        codex_version: "codex-cli-concurrent".into(),
        plan_type: "plus".into(),
        model_slug: "gpt-concurrent".into(),
        service_tier: "default".into(),
        schema_fingerprint: "fixture-concurrent".into(),
    };
    let barrier = Arc::new(Barrier::new(2));
    let handles = (0..2)
        .map(|_| {
            let state = state.clone();
            let identity = identity.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let mut store = StateStore::open_in(state, TrackerConfig::default()).unwrap();
                barrier.wait();
                store
                    .check_compatibility(identity, true, dt("2030-01-01T12:00:00Z"))
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let checks = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(checks.iter().filter(|check| check.first_seen).count(), 1);

    let store = StateStore::open_in(&state, TrackerConfig::default()).unwrap();
    let connection = rusqlite::Connection::open(&store.paths().database).unwrap();
    let result_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM compatibility_results", [], |row| {
            row.get(0)
        })
        .unwrap();
    let generation: String = connection
        .query_row(
            "SELECT current_compatibility_generation FROM config_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(result_count, 1);
    assert_eq!(generation, identity.key());
}

#[test]
fn unsupported_new_shape_degrades_projection_to_unknown_never_zero() {
    let temp = TempDir::new().unwrap();
    let transcript = temp.path().join("session.jsonl");
    fs::write(
        &transcript,
        format!(
            "{}\n",
            valid_line("2030-01-01T00:00:00Z", "gpt-old", "standard")
        ),
    )
    .unwrap();
    let mut store =
        StateStore::open_in(temp.path().join("state"), TrackerConfig::default()).unwrap();
    let options = IngestOptions {
        now: dt("2030-01-01T00:01:00Z"),
        future_tolerance: TimeDelta::minutes(5),
    };
    store.ingest_transcript(&transcript, &options).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .unwrap()
        .write_all(b"{\"timestamp\":\"2030-01-01T00:02:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"rate_limits\":\"moved\"}}\n")
        .unwrap();
    store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-01-01T00:03:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    let display = store
        .load_or_recover_display(dt("2030-01-01T00:03:00Z"))
        .unwrap();
    assert_eq!(display.status, WindowStatus::Unknown);
    assert_eq!(display.five_hour_estimate_percent, None);
    let (identity, supported) = store
        .current_compatibility_identity(Some("0.146.0"), None, None)
        .unwrap();
    assert!(!supported);
    let check = store
        .check_compatibility(identity, supported, dt("2030-01-01T00:04:00Z"))
        .unwrap();
    assert_eq!(check.result, CompatibilityResult::Degraded);
}

#[test]
fn release_metadata_is_allowlisted_and_cached_for_a_day() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("release.json");
    let marker = temp.path().join("must-not-run");
    let release = serde_json::json!({
        "tag_name": "rust-v0.200.0",
        "html_url": "https://github.com/openai/codex/releases/tag/rust-v0.200.0",
        "body": format!("$(touch {})", marker.display()),
    });
    fs::write(&source, serde_json::to_vec(&release).unwrap()).unwrap();
    unsafe { std::env::set_var("CODEX_USAGE_WATCH_RELEASE_METADATA_FILE", &source) };
    let first = cached_release_metadata(temp.path(), dt("2030-01-01T00:00:00Z"), true)
        .unwrap()
        .unwrap();
    fs::remove_file(&source).unwrap();
    let cached = cached_release_metadata(temp.path(), dt("2030-01-01T12:00:00Z"), true)
        .unwrap()
        .unwrap();
    unsafe { std::env::remove_var("CODEX_USAGE_WATCH_RELEASE_METADATA_FILE") };
    assert_eq!(first, cached);
    assert!(!marker.exists());
    #[cfg(unix)]
    {
        let mode = fs::metadata(temp.path().join("release-metadata.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn doctor_compat_runs_outside_hooks_and_never_blocks() {
    let temp = TempDir::new().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_codex-5h"))
        .args(["doctor", "--compat"])
        .env("CODEX_USAGE_WATCH_HOME", temp.path())
        .env("CODEX_VERSION", "0.146.0")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Codex version       0.146.0"));
    assert!(stdout.contains("Requests continue   yes"));
    assert!(stdout.contains("Result"));
}
