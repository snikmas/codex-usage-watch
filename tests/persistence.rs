use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;

use chrono::{DateTime, TimeDelta, Utc};
use codex_usage_watch::{
    DisplayCacheV1, IngestOptions, ObservationId, ResetClassification, StateStore, TrackerConfig,
    TranscriptCursor, WeeklySnapshot, WindowStatus, ingest_transcript,
};
use rusqlite::Connection;
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn dt(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn fixture_snapshots(name: &str) -> Vec<WeeklySnapshot> {
    ingest_transcript(
        &fixture(name),
        TranscriptCursor::default(),
        &IngestOptions {
            now: dt("2030-01-01T18:00:00Z"),
            future_tolerance: TimeDelta::minutes(5),
        },
    )
    .unwrap()
    .snapshots
}

fn ingest_reset_fixture(name: &str, now: &str) -> (TempDir, StateStore, DisplayCacheV1) {
    let temp = TempDir::new().unwrap();
    let transcript = temp.path().join(name);
    fs::copy(fixture(name), &transcript).unwrap();
    let mut store =
        StateStore::open_in(temp.path().join("state"), TrackerConfig::default()).unwrap();
    let outcome = store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt(now),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    (temp, store, outcome.persisted.display)
}

#[test]
fn server_five_hour_epochs_align_windows_and_restart_weekly_points() {
    let (_temp, store, display) =
        ingest_reset_fixture("reset_natural_five_hour.jsonl", "2030-01-01T13:02:00Z");
    assert_eq!(display.window_started_at, Some(dt("2030-01-01T13:00:00Z")));
    assert_eq!(display.window_ends_at, Some(dt("2030-01-01T18:00:00Z")));
    assert_eq!(
        display.window_boundary_kind.as_deref(),
        Some("natural_five_hour")
    );
    assert_eq!(display.window_boundary_at, Some(dt("2030-01-01T13:00:00Z")));
    assert_eq!(display.weekly_points, Some(1.0));
    assert_eq!(display.five_hour_estimate_percent, Some(2.0));
    assert_eq!(store.recent_windows(10).unwrap().len(), 2);
    let resets = store.recent_reset_events(10).unwrap();
    assert_eq!(resets.len(), 1);
    assert_eq!(
        resets[0].classification,
        ResetClassification::NaturalFiveHour
    );
}

#[test]
fn natural_weekly_rollover_stays_inside_the_same_five_hour_window() {
    let (_temp, store, display) =
        ingest_reset_fixture("reset_natural_weekly.jsonl", "2030-01-01T12:12:00Z");
    assert_eq!(display.window_started_at, Some(dt("2030-01-01T10:00:00Z")));
    assert_eq!(display.window_ends_at, Some(dt("2030-01-01T15:00:00Z")));
    assert_eq!(display.weekly_points, Some(3.0));
    assert_eq!(display.weekly_limit_used_percent, Some(2.0));
    assert_eq!(store.recent_windows(10).unwrap().len(), 1);
    assert_eq!(
        store.recent_reset_events(10).unwrap()[0].classification,
        ResetClassification::NaturalWeekly
    );
}

#[test]
fn early_full_reset_seeds_post_reset_usage_and_rebuilds_identically() {
    let (_temp, mut store, display) =
        ingest_reset_fixture("reset_early_full.jsonl", "2030-01-01T12:15:00Z");
    assert_eq!(display.window_started_at, Some(dt("2030-01-01T12:10:00Z")));
    assert_eq!(display.window_ends_at, Some(dt("2030-01-01T17:10:00Z")));
    assert_eq!(
        display.window_boundary_kind.as_deref(),
        Some("inferred_full")
    );
    assert_eq!(display.weekly_points, Some(5.0));
    assert_eq!(display.five_hour_estimate_percent, Some(4.0));
    let events = store.recent_reset_events(10).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].classification, ResetClassification::InferredFull);
    assert!(!events[0].reason.contains("user used"));

    let before = fs::read(&store.paths().display).unwrap();
    fs::remove_file(&store.paths().display).unwrap();
    let recovered = store
        .load_or_recover_display(dt("2030-01-01T12:15:00Z"))
        .unwrap();
    assert_eq!(recovered, display);
    assert_eq!(fs::read(&store.paths().display).unwrap(), before);
}

#[test]
fn reset_accounting_handles_non_decrease_zero_and_old_concurrent_epochs() {
    let (_temp, _store, non_decrease) = ingest_reset_fixture(
        "reset_early_full_non_decrease.jsonl",
        "2030-01-01T12:10:00Z",
    );
    assert_eq!(non_decrease.weekly_points, Some(2.0));
    assert_eq!(
        non_decrease.window_boundary_kind.as_deref(),
        Some("inferred_full")
    );

    let (_temp, _store, zero) =
        ingest_reset_fixture("reset_zero_then_growth.jsonl", "2030-01-01T12:15:00Z");
    assert_eq!(zero.weekly_points, Some(2.0));

    let (_temp, store, interleaved) =
        ingest_reset_fixture("reset_interleaved_old_epoch.jsonl", "2030-01-01T12:12:00Z");
    assert_eq!(interleaved.weekly_points, Some(4.0));
    let events = store.recent_reset_events(10).unwrap();
    assert!(
        events
            .iter()
            .any(|item| item.classification == ResetClassification::OldEpoch)
    );
    let connection = Connection::open(&store.paths().database).unwrap();
    let ignored: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM snapshots WHERE affects_meter = 0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ignored, 1);
}

#[test]
fn manual_reset_before_and_after_inferred_reset_is_durable_and_deterministic() {
    let temp = TempDir::new().unwrap();
    let transcript = temp.path().join("reset.jsonl");
    let lines: Vec<_> = fs::read_to_string(fixture("reset_early_full.jsonl"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    fs::write(&transcript, format!("{}\n", lines[0])).unwrap();
    let mut store =
        StateStore::open_in(temp.path().join("state"), TrackerConfig::default()).unwrap();
    let options = IngestOptions {
        now: dt("2030-01-01T12:05:00Z"),
        future_tolerance: TimeDelta::minutes(20),
    };
    store.ingest_transcript(&transcript, &options).unwrap();
    assert!(
        store
            .reset_current_window(dt("2030-01-01T12:05:00Z"))
            .unwrap()
    );

    fs::write(
        &transcript,
        format!("{}\n{}\n{}\n", lines[0], lines[1], lines[2]),
    )
    .unwrap();
    let outcome = store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-01-01T12:15:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    assert_eq!(
        outcome.persisted.display.window_boundary_kind.as_deref(),
        Some("inferred_full")
    );
    assert_eq!(store.recent_control_events(10).unwrap().len(), 1);
    assert_eq!(store.recent_reset_events(10).unwrap().len(), 1);

    assert!(
        store
            .reset_current_window(dt("2030-01-01T12:16:00Z"))
            .unwrap()
    );
    let duplicate = store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-01-01T12:17:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    assert_eq!(duplicate.inserted_observations, 0);
    assert_eq!(duplicate.persisted.display.status, WindowStatus::Unknown);
    assert_eq!(store.recent_control_events(10).unwrap().len(), 2);
    assert_eq!(store.recent_reset_events(10).unwrap().len(), 1);
}

#[test]
fn reset_history_survives_backup_restore_and_concurrent_refresh() {
    let temp = TempDir::new().unwrap();
    let transcript = temp.path().join("reset.jsonl");
    fs::copy(fixture("reset_early_full.jsonl"), &transcript).unwrap();
    let state = temp.path().join("state");
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let state = state.clone();
            let transcript = transcript.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let mut store = StateStore::open_in(state, TrackerConfig::default()).unwrap();
                barrier.wait();
                store
                    .ingest_transcript(
                        &transcript,
                        &IngestOptions {
                            now: dt("2030-01-01T12:15:00Z"),
                            future_tolerance: TimeDelta::minutes(5),
                        },
                    )
                    .unwrap();
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let store = StateStore::open_in(&state, TrackerConfig::default()).unwrap();
    assert_eq!(store.recent_reset_events(10).unwrap().len(), 1);
    assert_eq!(
        store
            .reset_classification_counts()
            .unwrap()
            .get("inferred_full"),
        Some(&1)
    );

    let backup = temp.path().join("backup.sqlite3");
    store.backup_database(&backup).unwrap();
    let restored = temp.path().join("restored");
    fs::create_dir_all(&restored).unwrap();
    fs::copy(&backup, restored.join("state.sqlite3")).unwrap();
    let restored = StateStore::open_in(restored, TrackerConfig::default()).unwrap();
    assert_eq!(restored.recent_reset_events(10).unwrap().len(), 1);
    assert_eq!(
        restored.recent_reset_events(10).unwrap()[0].classification,
        ResetClassification::InferredFull
    );
}

#[test]
fn ambiguous_missing_long_gap_and_jitter_cases_fail_open_without_overlap() {
    let cases = [
        ("reset_ambiguous_five_only.jsonl", "2030-01-01T12:10:00Z"),
        ("reset_missing_five_hour.jsonl", "2030-01-01T12:05:00Z"),
        ("reset_long_gap.jsonl", "2030-01-01T16:00:00Z"),
        ("reset_timestamp_jitter.jsonl", "2030-01-01T12:01:00Z"),
    ];
    for (fixture_name, now) in cases {
        let (_temp, store, display) = ingest_reset_fixture(fixture_name, now);
        let windows = store.recent_windows(100).unwrap();
        for pair in windows.iter().rev().collect::<Vec<_>>().windows(2) {
            assert!(pair[0].ends_at <= pair[1].started_at, "{fixture_name}");
        }
        match fixture_name {
            "reset_ambiguous_five_only.jsonl" => {
                assert_eq!(display.window_boundary_kind.as_deref(), Some("ambiguous"));
                assert_eq!(
                    store.recent_reset_events(10).unwrap()[0].classification,
                    ResetClassification::Ambiguous
                );
            }
            "reset_missing_five_hour.jsonl" => {
                assert_eq!(display.window_started_at, Some(dt("2030-01-01T12:00:00Z")));
                assert_eq!(display.window_ends_at, Some(dt("2030-01-01T17:00:00Z")));
                assert!(store.recent_reset_events(10).unwrap().is_empty());
            }
            "reset_long_gap.jsonl" => {
                assert_eq!(
                    store.recent_reset_events(10).unwrap()[0].classification,
                    ResetClassification::NaturalFiveHour
                );
            }
            "reset_timestamp_jitter.jsonl" => {
                assert_eq!(windows.len(), 1);
                assert!(store.recent_reset_events(10).unwrap().is_empty());
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn later_paired_data_realigns_earlier_weekly_only_observations() {
    let (_temp, store, display) =
        ingest_reset_fixture("reset_weekly_then_paired.jsonl", "2030-01-01T12:05:00Z");
    assert_eq!(display.window_started_at, Some(dt("2030-01-01T10:00:00Z")));
    assert_eq!(display.window_ends_at, Some(dt("2030-01-01T15:00:00Z")));
    assert_eq!(
        display.window_boundary_kind.as_deref(),
        Some("server_epoch")
    );
    assert_eq!(display.weekly_points, Some(2.0));
    assert_eq!(display.five_hour_estimate_percent, Some(7.0));
    assert_eq!(store.recent_windows(10).unwrap().len(), 1);
}

#[test]
fn warning_milestones_can_fire_again_in_a_new_server_epoch() {
    let temp = TempDir::new().unwrap();
    let transcript = temp.path().join("reset.jsonl");
    fs::copy(fixture("reset_natural_five_hour.jsonl"), &transcript).unwrap();
    let config = TrackerConfig::new(
        15.8,
        TimeDelta::hours(5),
        TimeDelta::minutes(15),
        vec![5],
        10,
        None,
    )
    .unwrap();
    let mut store = StateStore::open_in(temp.path().join("state"), config).unwrap();
    let outcome = store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-01-01T13:02:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    assert_eq!(outcome.persisted.newly_emitted_warnings, vec![5, 5]);
    let duplicate = store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-01-01T13:03:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    assert!(duplicate.persisted.newly_emitted_warnings.is_empty());
}

#[test]
fn incremental_and_full_replay_write_byte_equivalent_reset_projections() {
    for fixture_name in [
        "reset_natural_five_hour.jsonl",
        "reset_natural_weekly.jsonl",
        "reset_early_full.jsonl",
        "reset_early_full_non_decrease.jsonl",
        "reset_zero_then_growth.jsonl",
        "reset_interleaved_old_epoch.jsonl",
        "reset_ambiguous_five_only.jsonl",
        "reset_missing_five_hour.jsonl",
        "reset_long_gap.jsonl",
        "reset_timestamp_jitter.jsonl",
        "reset_weekly_then_paired.jsonl",
    ] {
        let temp = TempDir::new().unwrap();
        let full_path = temp.path().join("full.jsonl");
        let incremental_path = temp.path().join("incremental.jsonl");
        let contents = fs::read_to_string(fixture(fixture_name)).unwrap();
        fs::write(&full_path, &contents).unwrap();
        fs::write(&incremental_path, "").unwrap();
        let now = dt("2030-01-09T00:00:00Z");
        let options = IngestOptions {
            now,
            future_tolerance: TimeDelta::minutes(5),
        };
        let mut full =
            StateStore::open_in(temp.path().join("full-state"), TrackerConfig::default()).unwrap();
        full.ingest_transcript(&full_path, &options).unwrap();

        let mut incremental = StateStore::open_in(
            temp.path().join("incremental-state"),
            TrackerConfig::default(),
        )
        .unwrap();
        let mut prefix = String::new();
        for line in contents.lines() {
            prefix.push_str(line);
            prefix.push('\n');
            fs::write(&incremental_path, &prefix).unwrap();
            incremental
                .ingest_transcript(&incremental_path, &options)
                .unwrap();
        }
        assert_eq!(
            fs::read(&full.paths().display).unwrap(),
            fs::read(&incremental.paths().display).unwrap(),
            "{fixture_name}"
        );
        assert_eq!(
            full.recent_windows(100).unwrap(),
            incremental.recent_windows(100).unwrap(),
            "{fixture_name}"
        );
        let full_events = full.recent_reset_events(100).unwrap();
        let incremental_events = incremental.recent_reset_events(100).unwrap();
        assert_eq!(
            full_events
                .iter()
                .map(|item| (&item.classification, item.boundary_at))
                .collect::<Vec<_>>(),
            incremental_events
                .iter()
                .map(|item| (&item.classification, item.boundary_at))
                .collect::<Vec<_>>(),
            "{fixture_name}"
        );
    }
}

fn snapshot(source: &str, offset: u64, at: &str, used: f64) -> WeeklySnapshot {
    snapshot_with_reset(source, offset, at, used, "2030-01-08T00:00:00Z")
}

fn snapshot_with_reset(
    source: &str,
    offset: u64,
    at: &str,
    used: f64,
    reset: &str,
) -> WeeklySnapshot {
    WeeklySnapshot::new(
        ObservationId::new(source, offset),
        dt(at),
        used,
        Some(dt(reset)),
        10_080,
        Some("plus".to_owned()),
    )
    .unwrap()
}

#[test]
fn sqlite_replay_ignores_one_second_reset_jitter_and_older_epochs() {
    let temp = TempDir::new().unwrap();
    let mut store = StateStore::open_in(temp.path(), TrackerConfig::default()).unwrap();
    let outcome = store
        .ingest(
            [
                snapshot_with_reset("a", 0, "2030-01-01T12:00:00Z", 10.0, "2030-01-08T00:00:00Z"),
                snapshot_with_reset("b", 0, "2030-01-01T12:01:00Z", 17.0, "2030-01-08T00:00:01Z"),
                snapshot_with_reset("a", 1, "2030-01-01T12:02:00Z", 17.0, "2030-01-08T00:00:00Z"),
                snapshot_with_reset(
                    "new",
                    0,
                    "2030-01-01T12:03:00Z",
                    2.0,
                    "2030-01-15T00:00:00Z",
                ),
                snapshot_with_reset(
                    "old",
                    0,
                    "2030-01-01T12:04:00Z",
                    18.0,
                    "2030-01-08T00:00:00Z",
                ),
                snapshot_with_reset(
                    "new",
                    1,
                    "2030-01-01T12:05:00Z",
                    3.0,
                    "2030-01-15T00:00:00Z",
                ),
            ],
            dt("2030-01-01T12:05:00Z"),
        )
        .unwrap();

    assert_eq!(outcome.display.weekly_points, Some(10.0));
    let connection = Connection::open(&store.paths().database).unwrap();
    let ignored: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM snapshots WHERE affects_meter = 0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ignored, 1);
}

#[test]
fn migration_creates_the_stage_three_schema_and_wal_database() {
    let temp = TempDir::new().unwrap();
    let store = StateStore::open_in(temp.path(), TrackerConfig::default()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 11);

    let connection = Connection::open(&store.paths().database).unwrap();
    let tables: Vec<String> = connection
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for required in [
        "config_metadata",
        "control_events",
        "emitted_warnings",
        "diagnostic_events",
        "observed_rate_limit_windows",
        "rate_limit_observations",
        "reset_events",
        "schema_migrations",
        "snapshots",
        "transcript_cursors",
        "windows",
    ] {
        assert!(tables.iter().any(|table| table == required));
    }
    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode, "wal");
}

#[cfg(unix)]
#[test]
fn startup_repairs_private_state_permissions_without_touching_the_parent() {
    let temp = TempDir::new().unwrap();
    let parent = temp.path().join("user-selected-parent");
    let state = parent.join("tracker-state");
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();

    let mut store = StateStore::open_in(&state, TrackerConfig::default()).unwrap();
    store
        .ingest(
            fixture_snapshots("normal_growth.jsonl"),
            dt("2030-01-01T12:10:00Z"),
        )
        .unwrap();
    fs::write(state.join("calibration-report.json"), b"{}\n").unwrap();
    fs::write(state.join("release-metadata.json"), b"{}\n").unwrap();
    for path in [
        state.join("state.sqlite3"),
        state.join("display.json"),
        state.join("calibration-report.json"),
        state.join("release-metadata.json"),
    ] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
    }
    fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
    drop(store);

    let store = StateStore::open_in(&state, TrackerConfig::default()).unwrap();
    assert_eq!(
        fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for path in [
        state.join("state.sqlite3"),
        state.join("display.json"),
        state.join("calibration-report.json"),
        state.join("release-metadata.json"),
    ] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let backup = parent.join("backup.sqlite3");
    store.backup_database(&backup).unwrap();
    assert_eq!(
        fs::metadata(backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn schema_v1_migrates_without_losing_snapshots_or_windows() {
    let temp = TempDir::new().unwrap();
    let database = temp.path().join("state.sqlite3");
    let connection = Connection::open(&database).unwrap();
    connection.execute_batch(
        "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);
         INSERT INTO schema_migrations VALUES (1, '2030-01-01T00:00:00Z');
         CREATE TABLE config_metadata (
             singleton INTEGER PRIMARY KEY, calibration_weekly_points REAL NOT NULL,
             local_window_seconds INTEGER NOT NULL, stale_after_seconds INTEGER NOT NULL,
             warning_thresholds_json TEXT NOT NULL, super_usage_step INTEGER NOT NULL,
             calibration_kind TEXT NOT NULL, updated_at TEXT NOT NULL);
         INSERT INTO config_metadata VALUES
             (1, 15.8, 18000, 900, '[75,90,100]', 10, 'historical', '2030-01-01T00:00:00Z');
         CREATE TABLE snapshots (
             id INTEGER PRIMARY KEY, source_file TEXT NOT NULL, byte_offset INTEGER NOT NULL,
             observed_at TEXT NOT NULL, used_percent REAL NOT NULL, resets_at TEXT,
             window_minutes INTEGER NOT NULL, plan_type TEXT, affects_meter INTEGER NOT NULL DEFAULT 0,
             UNIQUE(source_file, byte_offset));
         CREATE INDEX snapshots_order ON snapshots(observed_at, source_file, byte_offset);
         INSERT INTO snapshots VALUES
             (7, 'old.jsonl', 12, '2030-01-01T12:00:00Z', 20.0,
              '2030-01-08T00:00:00Z', 10080, 'plus', 1);
         CREATE TABLE windows (
             started_at TEXT PRIMARY KEY, ends_at TEXT NOT NULL,
             latest_observed_at TEXT NOT NULL, latest_used_percent REAL NOT NULL,
             calibration_weekly_points REAL NOT NULL, accumulated_weekly_points REAL NOT NULL,
             last_emitted_milestone INTEGER, lifecycle TEXT NOT NULL);
         CREATE UNIQUE INDEX one_current_window ON windows(lifecycle) WHERE lifecycle = 'current';
         INSERT INTO windows VALUES
             ('2030-01-01T12:00:00Z', '2030-01-01T17:00:00Z',
              '2030-01-01T12:00:00Z', 20.0, 15.8, 3.5, NULL, 'current');
         CREATE TABLE emitted_warnings (
             window_started_at TEXT NOT NULL, milestone INTEGER NOT NULL,
             emitted_at TEXT NOT NULL, PRIMARY KEY(window_started_at, milestone));
         PRAGMA user_version = 1;",
    )
    .unwrap();
    drop(connection);

    let store = StateStore::open_in(temp.path(), TrackerConfig::default()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 11);
    assert_eq!(store.snapshot_count().unwrap(), 1);
    let connection = Connection::open(&database).unwrap();
    let window: (f64, f64) = connection
        .query_row(
            "SELECT calibration_weekly_points, accumulated_weekly_points FROM windows",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(window, (15.8, 3.5));
}

#[test]
fn persisted_cursor_makes_unchanged_ingest_empty_and_truncation_safe() {
    let temp = TempDir::new().unwrap();
    let transcript = temp.path().join("session.jsonl");
    fs::copy(fixture("real_dual_window.jsonl"), &transcript).unwrap();
    let state = temp.path().join("state");
    let mut store = StateStore::open_in(&state, TrackerConfig::default()).unwrap();
    let options = IngestOptions {
        now: dt("2030-01-01T18:00:00Z"),
        future_tolerance: TimeDelta::minutes(5),
    };

    let first = store.ingest_transcript(&transcript, &options).unwrap();
    assert_eq!(first.inserted_observations, 2);
    assert_eq!(store.observation_count().unwrap(), 2);
    let connection = Connection::open(&store.paths().database).unwrap();
    let retained: (String, String, i64) = connection
        .query_row(
            "SELECT plan_type, schema_fingerprint,
                    (SELECT COUNT(*) FROM observed_rate_limit_windows)
             FROM rate_limit_observations LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(retained.0, "plus");
    assert!(retained.1.contains("window_minutes:number=300"));
    assert_eq!(retained.2, 4);
    drop(connection);
    let saved = store.transcript_cursor(&transcript).unwrap();
    assert!(saved.next_offset > 0);

    let second = store.ingest_transcript(&transcript, &options).unwrap();
    assert_eq!(second.batch.start_offset, saved.next_offset);
    assert!(second.batch.observations.is_empty());
    assert_eq!(second.inserted_observations, 0);

    let one_line = include_str!("fixtures/normal_growth.jsonl")
        .lines()
        .next()
        .unwrap();
    fs::write(&transcript, format!("{one_line}\n")).unwrap();
    let after_truncate = store.ingest_transcript(&transcript, &options).unwrap();
    assert_eq!(after_truncate.batch.start_offset, 0);
    assert_eq!(after_truncate.batch.observations.len(), 1);
    assert_eq!(store.diagnostic_count().unwrap(), 1);
    assert!(store.transcript_cursor(&transcript).unwrap().next_offset > 0);

    // Regrowing with the exact committed prefix is safely incremental: the
    // retained first record is identical, so only the nine appended records
    // need ingestion.
    let replacement = format!("{one_line}\n").repeat(10);
    fs::write(&transcript, replacement).unwrap();
    let after_regrow = store.ingest_transcript(&transcript, &options).unwrap();
    assert_eq!(after_regrow.batch.start_offset, 219);
    assert_eq!(after_regrow.batch.observations.len(), 9);
    assert_eq!(after_regrow.inserted_observations, 9);
    assert_eq!(store.observation_count().unwrap(), 12);

    // A replacement whose committed prefix differs must restart even when it
    // has already regrown beyond the saved cursor. Its generation-qualified
    // identities retain both histories without path/offset collisions.
    let different_line = include_str!("fixtures/real_dual_window.jsonl")
        .lines()
        .next()
        .unwrap();
    fs::write(&transcript, format!("{different_line}\n").repeat(12)).unwrap();
    let after_different_regrow = store.ingest_transcript(&transcript, &options).unwrap();
    assert_eq!(after_different_regrow.batch.start_offset, 0);
    assert_eq!(after_different_regrow.batch.observations.len(), 12);
    assert_eq!(after_different_regrow.inserted_observations, 12);
    assert_eq!(store.observation_count().unwrap(), 24);
}

#[test]
fn overlapping_independent_writers_are_idempotent_and_order_independent() {
    let temp = TempDir::new().unwrap();
    let directory = temp.path().to_path_buf();
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = ["concurrent_a.jsonl", "concurrent_b.jsonl"]
        .into_iter()
        .map(|fixture_name| {
            let directory = directory.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let snapshots = fixture_snapshots(fixture_name);
                barrier.wait();
                let mut store = StateStore::open_in(directory, TrackerConfig::default()).unwrap();
                store.ingest(snapshots, dt("2030-01-01T12:03:00Z")).unwrap()
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let mut store = StateStore::open_in(&directory, TrackerConfig::default()).unwrap();
    assert_eq!(store.snapshot_count().unwrap(), 4);
    let display = store
        .load_or_recover_display(dt("2030-01-01T12:03:00Z"))
        .unwrap();
    assert_eq!(display.weekly_points, Some(2.0));
    assert_eq!(display.five_hour_estimate_percent, Some(2.0 / 15.8 * 100.0));

    let duplicate = store
        .ingest(
            fixture_snapshots("concurrent_a.jsonl"),
            dt("2030-01-01T12:03:00Z"),
        )
        .unwrap();
    assert_eq!(duplicate.inserted_snapshots, 0);
    assert_eq!(duplicate.display.weekly_points, Some(2.0));

    let connection = Connection::open(&store.paths().database).unwrap();
    let integrity: String = connection
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
}

#[test]
fn explicit_reset_archives_with_audit_and_next_observation_starts_a_new_window() {
    let temp = TempDir::new().unwrap();
    let mut store = StateStore::open_in(temp.path(), TrackerConfig::default()).unwrap();
    store
        .ingest(
            fixture_snapshots("normal_growth.jsonl"),
            dt("2030-01-01T12:10:00Z"),
        )
        .unwrap();

    assert!(
        store
            .reset_current_window(dt("2030-01-01T12:15:00Z"))
            .unwrap()
    );
    let reset_display = store
        .load_or_recover_display(dt("2030-01-01T12:15:00Z"))
        .unwrap();
    assert_eq!(reset_display.status, WindowStatus::Unknown);
    let events = store.recent_control_events(10).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "manual_reset");

    store
        .ingest(
            [snapshot(
                "after-reset.jsonl",
                1,
                "2030-01-01T12:16:00Z",
                23.0,
            )],
            dt("2030-01-01T12:16:00Z"),
        )
        .unwrap();
    let windows = store.recent_windows(10).unwrap();
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0].started_at, dt("2030-01-01T12:16:00Z"));
    assert_eq!(windows[0].lifecycle, "current");
    assert_eq!(windows[1].ends_at, dt("2030-01-01T12:15:00Z"));
    assert_eq!(windows[1].lifecycle, "archived");
}

#[test]
fn display_projection_has_versioned_used_left_freshness_and_calibration_fields() {
    let temp = TempDir::new().unwrap();
    let mut store = StateStore::open_in(temp.path(), TrackerConfig::default()).unwrap();
    let outcome = store
        .ingest(
            fixture_snapshots("normal_growth.jsonl"),
            dt("2030-01-01T12:10:00Z"),
        )
        .unwrap();

    let display = outcome.display;
    assert_eq!(display.schema_version, 2);
    assert_eq!(display.status, WindowStatus::Fresh);
    assert!(!display.stale);
    assert_eq!(display.data_age_seconds, Some(300));
    assert_eq!(display.weekly_points, Some(2.0));
    assert_eq!(display.weekly_limit_used_percent, Some(22.0));
    assert_eq!(display.weekly_limit_left_percent, Some(78.0));
    assert_eq!(display.calibration_weekly_points, Some(15.8));
    assert_eq!(
        display.five_hour_value_source.as_deref(),
        Some("local_calibrated_estimate")
    );
    assert_eq!(
        display.five_hour_estimate_left_percent,
        Some(100.0 - (2.0 / 15.8 * 100.0))
    );

    let disk: DisplayCacheV1 =
        serde_json::from_slice(&fs::read(&store.paths().display).unwrap()).unwrap();
    assert_eq!(disk, display);
    assert!(fs::read_dir(temp.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")
    }));
}

#[test]
fn read_only_display_refreshes_freshness_without_rewriting_the_projection() {
    let temp = TempDir::new().unwrap();
    let config = TrackerConfig::new(
        15.8,
        TimeDelta::hours(5),
        TimeDelta::minutes(15),
        vec![75, 90, 100],
        10,
        Some(temp.path().to_path_buf()),
    )
    .unwrap();
    let mut store = StateStore::open(config.clone()).unwrap();
    store
        .ingest(
            fixture_snapshots("normal_growth.jsonl"),
            dt("2030-01-01T12:10:00Z"),
        )
        .unwrap();
    let before = fs::read(&store.paths().display).unwrap();
    drop(store);

    let display = StateStore::load_display_read_only(&config, dt("2030-01-01T12:21:00Z")).unwrap();
    assert_eq!(display.status, WindowStatus::Stale);
    assert!(display.stale);
    assert_eq!(display.data_age_seconds, Some(16 * 60));
    assert_eq!(fs::read(temp.path().join("display.json")).unwrap(), before);
}

#[test]
fn fresh_real_five_hour_window_has_priority_over_local_estimate() {
    let temp = TempDir::new().unwrap();
    let transcript = temp.path().join("dual.jsonl");
    fs::copy(fixture("real_dual_window.jsonl"), &transcript).unwrap();
    let mut store =
        StateStore::open_in(temp.path().join("state"), TrackerConfig::default()).unwrap();
    let outcome = store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-01-01T12:10:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    assert_eq!(
        outcome.persisted.display.five_hour_estimate_percent,
        Some(1.0)
    );
    assert_eq!(
        outcome.persisted.display.five_hour_value_source.as_deref(),
        Some("real_server_five_hour")
    );
}

#[test]
fn missing_or_malformed_display_is_recovered_from_sqlite() {
    let temp = TempDir::new().unwrap();
    let mut store = StateStore::open_in(temp.path(), TrackerConfig::default()).unwrap();
    store
        .ingest(
            fixture_snapshots("weekly_reset.jsonl"),
            dt("2030-01-01T12:15:00Z"),
        )
        .unwrap();
    fs::write(&store.paths().display, b"not json").unwrap();

    let recovered = store
        .load_or_recover_display(dt("2030-01-01T12:15:00Z"))
        .unwrap();
    assert_eq!(recovered.weekly_points, Some(4.0));
    serde_json::from_slice::<DisplayCacheV1>(&fs::read(&store.paths().display).unwrap()).unwrap();

    fs::remove_file(&store.paths().display).unwrap();
    let recovered_again = store
        .load_or_recover_display(dt("2030-01-01T12:16:00Z"))
        .unwrap();
    assert_eq!(recovered_again.weekly_points, Some(4.0));

    let mut unsupported_cache = serde_json::to_value(&recovered_again).unwrap();
    unsupported_cache["schema_version"] = serde_json::json!(999);
    fs::write(
        &store.paths().display,
        serde_json::to_vec(&unsupported_cache).unwrap(),
    )
    .unwrap();
    let regenerated = store
        .load_or_recover_display(dt("2030-01-01T12:17:00Z"))
        .unwrap();
    assert_eq!(regenerated.schema_version, 2);
    assert_eq!(regenerated.weekly_points, Some(4.0));
}

#[test]
fn newer_database_schema_is_rejected_without_mutating_state() {
    let temp = TempDir::new().unwrap();
    let store = StateStore::open_in(temp.path(), TrackerConfig::default()).unwrap();
    let database = store.paths().database.clone();
    drop(store);
    let connection = Connection::open(&database).unwrap();
    connection.pragma_update(None, "user_version", 999).unwrap();
    drop(connection);
    let error = StateStore::open_in(temp.path(), TrackerConfig::default())
        .err()
        .expect("newer schema must be rejected");
    assert!(error.to_string().contains("newer than supported"));
    let connection = Connection::open(database).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 999);
}

#[test]
fn unknown_stale_super_usage_and_warning_audit_are_preserved() {
    let temp = TempDir::new().unwrap();
    let mut store = StateStore::open_in(temp.path(), TrackerConfig::default()).unwrap();
    let unknown = store
        .load_or_recover_display(dt("2030-01-01T12:00:00Z"))
        .unwrap();
    assert_eq!(unknown.status, WindowStatus::Unknown);
    assert_eq!(unknown.five_hour_estimate_percent, None);

    let first = store
        .ingest(
            [
                snapshot("warnings", 0, "2030-01-01T12:00:00Z", 0.0),
                snapshot("warnings", 1, "2030-01-01T12:01:00Z", 18.0),
            ],
            dt("2030-01-01T12:01:00Z"),
        )
        .unwrap();
    assert_eq!(first.newly_emitted_warnings, vec![110]);
    assert!(first.display.five_hour_estimate_percent.unwrap() > 100.0);
    assert_eq!(first.display.five_hour_estimate_left_percent, Some(0.0));

    let duplicate = store
        .ingest(
            [snapshot("warnings", 1, "2030-01-01T12:01:00Z", 18.0)],
            dt("2030-01-01T12:02:00Z"),
        )
        .unwrap();
    assert!(duplicate.newly_emitted_warnings.is_empty());

    let stale = store
        .load_or_recover_display(dt("2030-01-01T12:17:00Z"))
        .unwrap();
    // A valid cache is intentionally stable until a refresh regenerates it.
    assert_eq!(stale.generated_at, dt("2030-01-01T12:02:00Z"));
    let refreshed = store.ingest([], dt("2030-01-01T12:17:00Z")).unwrap();
    assert_eq!(refreshed.display.status, WindowStatus::Stale);
    assert!(refreshed.display.stale);
}
