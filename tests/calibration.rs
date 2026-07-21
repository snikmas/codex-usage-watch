use std::fs;
use std::io::Write;

use chrono::{DateTime, TimeDelta, Utc};
use codex_usage_watch::{
    CalibrationConfidence, EvidenceQuality, GroundTruthStatus, IngestOptions, StateStore,
    TrackerConfig, WindowStatus,
};
use rusqlite::Connection;
use tempfile::TempDir;

fn dt(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn token_line(at: &str, five_used: f64, five_reset: i64, weekly_used: f64) -> String {
    format!(
        "{{\"timestamp\":\"{at}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"model\":\"gpt-test\",\"service_tier\":\"standard\",\"rate_limits\":{{\"plan_type\":\"plus\",\"primary\":{{\"used_percent\":{five_used},\"window_minutes\":300,\"resets_at\":{five_reset}}},\"secondary\":{{\"used_percent\":{weekly_used},\"window_minutes\":10080,\"resets_at\":1894060800}}}}}}}}"
    )
}

fn token_line_with_resets(
    at: &str,
    five_used: f64,
    five_reset: &str,
    weekly_used: f64,
    weekly_reset: &str,
) -> String {
    format!(
        "{{\"timestamp\":\"{at}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"model\":\"gpt-test\",\"service_tier\":\"standard\",\"rate_limits\":{{\"plan_type\":\"plus\",\"primary\":{{\"used_percent\":{five_used},\"window_minutes\":300,\"resets_at\":\"{five_reset}\"}},\"secondary\":{{\"used_percent\":{weekly_used},\"window_minutes\":10080,\"resets_at\":\"{weekly_reset}\"}}}}}}}}"
    )
}

#[test]
fn calibration_normalizes_jitter_but_never_spans_a_weekly_reset() {
    let temp = TempDir::new().unwrap();
    let transcript = temp.path().join("boundaries.jsonl");
    let lines = [
        token_line_with_resets(
            "2030-01-01T00:00:00Z",
            0.0,
            "2030-01-01T05:00:00Z",
            10.0,
            "2030-01-01T02:00:00Z",
        ),
        token_line_with_resets(
            "2030-01-01T01:00:00Z",
            50.0,
            "2030-01-01T05:00:01Z",
            20.0,
            "2030-01-01T02:00:01Z",
        ),
        token_line_with_resets(
            "2030-01-01T02:01:00Z",
            50.0,
            "2030-01-01T05:00:00Z",
            1.0,
            "2030-01-08T02:00:00Z",
        ),
        token_line_with_resets(
            "2030-01-01T03:00:00Z",
            100.0,
            "2030-01-01T05:00:00Z",
            11.0,
            "2030-01-08T02:00:00Z",
        ),
    ];
    fs::write(&transcript, format!("{}\n", lines.join("\n"))).unwrap();
    let mut store =
        StateStore::open_in(temp.path().join("state"), TrackerConfig::default()).unwrap();
    store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-01-01T03:01:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    let report = store
        .analyze_calibration(dt("2030-01-01T03:02:00Z"))
        .unwrap();
    assert_eq!(report.total_completed_window_count, 2);
    assert_eq!(
        report.samples[0].window_ended_at,
        dt("2030-01-01T01:00:00Z")
    );
    assert_eq!(
        report.samples[1].window_started_at,
        dt("2030-01-01T02:01:00Z")
    );
    assert_ne!(
        report.samples[0].weekly_reset_at,
        report.samples[1].weekly_reset_at
    );
}

#[test]
fn five_windows_produce_robust_idempotent_report_and_no_auto_apply() {
    let temp = TempDir::new().unwrap();
    let transcript = temp.path().join("calibration.jsonl");
    let starts = [
        ("2030-01-01T00:00:00Z", "2030-01-01T01:00:00Z", 1893474000),
        ("2030-01-02T00:00:00Z", "2030-01-02T01:00:00Z", 1893560400),
        ("2030-01-03T00:00:00Z", "2030-01-03T01:00:00Z", 1893646800),
        ("2030-01-04T00:00:00Z", "2030-01-04T01:00:00Z", 1893733200),
        ("2030-01-05T00:00:00Z", "2030-01-05T01:00:00Z", 1893819600),
    ];
    let mut lines = Vec::new();
    for (index, (start, end, reset)) in starts.into_iter().enumerate() {
        lines.push(token_line(start, 0.0, reset, 10.0));
        let weekly_end = if index == 4 { 60.0 } else { 20.0 };
        lines.push(token_line(end, 50.0, reset, weekly_end));
    }
    fs::write(&transcript, format!("{}\n", lines.join("\n"))).unwrap();

    let mut store =
        StateStore::open_in(temp.path().join("state"), TrackerConfig::default()).unwrap();
    let options = IngestOptions {
        now: dt("2030-01-06T00:00:00Z"),
        future_tolerance: TimeDelta::minutes(5),
    };
    store.ingest_transcript(&transcript, &options).unwrap();
    let analyzed_at = dt("2030-01-06T00:01:00Z");
    let first = store.analyze_calibration(analyzed_at).unwrap();
    let second = store.analyze_calibration(analyzed_at).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.sample_count, 5);
    assert_eq!(first.weighted_median, Some(20.0));
    assert_eq!(first.maximum, Some(100.0));
    assert_eq!(first.outlier_count, 1);
    assert_eq!(first.proposed_calibration, Some(20.0));
    assert!((first.samples[0].predicted_five_hour_percent.unwrap() - 63.291).abs() < 0.01);
    assert!((first.samples[0].prediction_error_percent.unwrap() - 13.291).abs() < 0.01);
    assert!(!first.recommend_change);
    assert_eq!(first.confidence, CalibrationConfidence::PersonalCandidate);
    assert_eq!(first.ground_truth_status, GroundTruthStatus::Measured);
    assert_eq!(store.calibration_sample_count().unwrap(), 5);
    assert_eq!(store.active_calibration(), 15.8);

    let sixth = [
        token_line("2030-01-06T02:00:00Z", 0.0, 1893906000, 10.0),
        token_line("2030-01-06T03:00:00Z", 50.0, 1893906000, 20.0),
    ];
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap(),
        "{}",
        sixth.join("\n")
    )
    .unwrap();
    store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-01-06T04:00:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    let confirmed = store
        .analyze_calibration(dt("2030-01-06T04:01:00Z"))
        .unwrap();
    assert_eq!(confirmed.drift_confirmation_count, 2);
    assert!(confirmed.recommend_change);
}

#[test]
fn weekly_only_reports_no_ground_truth_and_apply_changes_future_windows_only() {
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("state");
    let mut store = StateStore::open_in(&state, TrackerConfig::default()).unwrap();
    let transcript = temp.path().join("weekly.jsonl");
    let lines = [
        "{\"timestamp\":\"2030-01-01T00:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"rate_limits\":{\"plan_type\":\"plus\",\"primary\":{\"used_percent\":10,\"window_minutes\":10080,\"resets_at\":1894060800}}}}",
        "{\"timestamp\":\"2030-01-01T01:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"rate_limits\":{\"plan_type\":\"plus\",\"primary\":{\"used_percent\":12,\"window_minutes\":10080,\"resets_at\":1894060800}}}}",
        "{\"timestamp\":\"2030-01-01T06:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"rate_limits\":{\"plan_type\":\"plus\",\"primary\":{\"used_percent\":14,\"window_minutes\":10080,\"resets_at\":1894060800}}}}",
    ];
    fs::write(&transcript, format!("{}\n", lines.join("\n"))).unwrap();
    store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-01-01T07:00:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    let report = store
        .analyze_calibration(dt("2030-01-01T07:01:00Z"))
        .unwrap();
    assert_eq!(
        report.ground_truth_status,
        GroundTruthStatus::NoNewGroundTruth
    );
    assert_eq!(report.weekly_only_observation_count, 3);
    assert_eq!(report.proposed_calibration, None);

    store
        .apply_calibration(20.0, dt("2030-01-01T07:02:00Z"))
        .unwrap();
    writeln!(
        fs::OpenOptions::new().append(true).open(&transcript).unwrap(),
        "{{\"timestamp\":\"2030-01-01T12:00:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"rate_limits\":{{\"plan_type\":\"plus\",\"primary\":{{\"used_percent\":16,\"window_minutes\":10080,\"resets_at\":1894060800}}}}}}}}"
    )
    .unwrap();
    store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-01-01T12:01:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    let reopened = StateStore::open_in(&state, TrackerConfig::default()).unwrap();
    assert_eq!(reopened.active_calibration(), 20.0);
    let connection = Connection::open(reopened.paths().database.clone()).unwrap();
    let calibrations: Vec<(f64, String, String)> = connection
        .prepare(
            "SELECT calibration_weekly_points, calibration_id, calibration_confidence
             FROM windows ORDER BY started_at",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        calibrations.iter().map(|item| item.0).collect::<Vec<_>>(),
        vec![15.8, 15.8, 20.0]
    );
    assert_eq!(calibrations[0].1, calibrations[1].1);
    assert_ne!(calibrations[1].1, calibrations[2].1);
    assert_eq!(calibrations[2].2, "personal_candidate");
    assert_eq!(
        reopened
            .selected_calibration_profile()
            .unwrap()
            .calibration_id,
        calibrations[2].1
    );
}

#[test]
fn quality_boundaries_exclude_tiny_and_low_volume_movements_deterministically() {
    let temp = TempDir::new().unwrap();
    let transcript = temp.path().join("quality.jsonl");
    let movements = [24.9, 25.0, 49.9, 50.0, 79.9, 80.0];
    let mut lines = Vec::new();
    for (index, movement) in movements.into_iter().enumerate() {
        let day = index + 1;
        let start = format!("2030-02-{day:02}T00:00:00Z");
        let end = format!("2030-02-{day:02}T01:00:00Z");
        let reset = dt(&start).timestamp() + 18_000;
        lines.push(token_line(&start, 0.0, reset, 10.0));
        lines.push(token_line(&end, movement, reset, 10.0 + movement * 0.2));
    }
    fs::write(&transcript, format!("{}\n", lines.join("\n"))).unwrap();
    let mut store =
        StateStore::open_in(temp.path().join("state"), TrackerConfig::default()).unwrap();
    store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-02-07T00:00:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    let report = store
        .analyze_calibration(dt("2030-02-07T00:01:00Z"))
        .unwrap();
    assert_eq!(report.ignored_count, 1);
    assert_eq!(report.low_quality_count, 2);
    assert_eq!(report.useful_count, 2);
    assert_eq!(report.high_quality_count, 1);
    assert_eq!(report.sample_count, 3);
    assert_eq!(report.weighted_median, Some(20.0));
    assert_eq!(report.proposed_calibration, None);
    assert_eq!(report.samples[0].quality, EvidenceQuality::Ignored);
    assert!(report.samples[0].diagnostic_reason.contains("ignored"));
}

#[test]
fn unknown_or_incompatible_plan_never_receives_the_plus_baseline() {
    let temp = TempDir::new().unwrap();
    let transcript = temp.path().join("unknown-plan.jsonl");
    let line = token_line("2030-03-01T00:00:00Z", 50.0, 1898744400, 20.0)
        .replace("\"plan_type\":\"plus\",", "");
    fs::write(&transcript, format!("{line}\n")).unwrap();
    let mut store =
        StateStore::open_in(temp.path().join("state"), TrackerConfig::default()).unwrap();
    store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-03-01T00:01:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    let profile = store.selected_calibration_profile().unwrap();
    assert_eq!(profile.confidence, CalibrationConfidence::Unsupported);
    assert_eq!(profile.value, None);
    assert!(
        store
            .apply_calibration(15.8, dt("2030-03-01T00:02:00Z"))
            .is_err()
    );
    let display = store
        .load_or_recover_display(dt("2030-03-01T00:02:00Z"))
        .unwrap();
    assert_eq!(display.status, WindowStatus::Unknown);
    assert_eq!(display.five_hour_estimate_percent, None);
}

#[test]
fn scheduled_reports_are_initial_then_weekly_without_duplicates() {
    let temp = TempDir::new().unwrap();
    let mut store = StateStore::open_in(temp.path(), TrackerConfig::default()).unwrap();
    let first = store
        .maybe_generate_calibration_report(dt("2030-04-01T00:00:00Z"))
        .unwrap();
    let repeated = store
        .maybe_generate_calibration_report(dt("2030-04-01T01:00:00Z"))
        .unwrap();
    let weekly = store
        .maybe_generate_calibration_report(dt("2030-04-08T00:00:00Z"))
        .unwrap();
    assert_eq!(
        first.as_ref().map(|value| value.0.as_str()),
        Some("initial")
    );
    assert!(repeated.is_none());
    assert_eq!(
        weekly.as_ref().map(|value| value.0.as_str()),
        Some("weekly")
    );
    assert!(store.paths().calibration_report.exists());
    let connection = Connection::open(store.paths().database.clone()).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM calibration_reports", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn five_new_qualifying_windows_produce_an_early_report() {
    let temp = TempDir::new().unwrap();
    let transcript = temp.path().join("early.jsonl");
    let starts = [
        ("2030-05-01T00:00:00Z", "2030-05-01T01:00:00Z", 1903861200),
        ("2030-05-02T00:00:00Z", "2030-05-02T01:00:00Z", 1903947600),
        ("2030-05-03T00:00:00Z", "2030-05-03T01:00:00Z", 1904034000),
        ("2030-05-04T00:00:00Z", "2030-05-04T01:00:00Z", 1904120400),
        ("2030-05-05T00:00:00Z", "2030-05-05T01:00:00Z", 1904206800),
    ];
    let initial_lines: Vec<String> = starts
        .iter()
        .map(|(start, _, reset)| token_line(start, 0.0, *reset, 10.0))
        .collect();
    fs::write(&transcript, format!("{}\n", initial_lines.join("\n"))).unwrap();
    let mut store =
        StateStore::open_in(temp.path().join("state"), TrackerConfig::default()).unwrap();
    store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-05-06T00:00:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    let initial = store
        .maybe_generate_calibration_report(dt("2030-05-06T00:01:00Z"))
        .unwrap();
    assert_eq!(initial.unwrap().0, "initial");

    let ending_lines: Vec<String> = starts
        .iter()
        .map(|(_, end, reset)| token_line(end, 50.0, *reset, 20.0))
        .collect();
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap(),
        "{}",
        ending_lines.join("\n")
    )
    .unwrap();
    store
        .ingest_transcript(
            &transcript,
            &IngestOptions {
                now: dt("2030-05-06T00:02:00Z"),
                future_tolerance: TimeDelta::minutes(5),
            },
        )
        .unwrap();
    let early = store
        .maybe_generate_calibration_report(dt("2030-05-06T00:03:00Z"))
        .unwrap()
        .unwrap();
    assert_eq!(early.0, "five_new_qualifying_windows");
    assert_eq!(early.1.sample_count, 5);
}
