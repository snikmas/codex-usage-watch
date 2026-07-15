use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};
use codex_usage_watch::{
    DiscoveryOptions, HookInput, IngestOptions, TranscriptCursor, discover_recent_transcripts,
    ingest_hook_transcript, ingest_transcript,
};
use tempfile::TempDir;

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

fn fixture_options() -> IngestOptions {
    IngestOptions {
        now: dt("2030-01-01T18:00:00Z"),
        future_tolerance: TimeDelta::minutes(5),
    }
}

#[test]
fn parses_weekly_only_primary_shape_and_stable_offsets() {
    let path = fixture("real_weekly_only.jsonl");
    let batch = ingest_transcript(&path, TranscriptCursor::default(), &fixture_options()).unwrap();
    let canonical = fs::canonicalize(path).unwrap();

    assert_eq!(batch.snapshots.len(), 2);
    assert_eq!(batch.snapshots[0].used_percent, 5.0);
    assert_eq!(batch.snapshots[1].used_percent, 5.0);
    assert_eq!(batch.snapshots[0].id.source_file, canonical);
    assert_eq!(batch.snapshots[0].id.byte_offset, 0);
    assert!(batch.snapshots[1].id.byte_offset > 0);
    assert_eq!(batch.snapshots[0].window_minutes, 10_080);
    assert!(!batch.incomplete_tail);
}

#[test]
fn selects_the_weekly_secondary_from_historical_dual_window_shape() {
    let batch = ingest_transcript(
        &fixture("real_dual_window.jsonl"),
        TranscriptCursor::default(),
        &fixture_options(),
    )
    .unwrap();

    let values: Vec<_> = batch
        .snapshots
        .iter()
        .map(|value| value.used_percent)
        .collect();
    assert_eq!(values, vec![56.0, 57.0]);
    assert_eq!(batch.snapshots[0].window_minutes, 10_080);
    assert_eq!(
        batch.observations[0]
            .five_hour
            .as_ref()
            .unwrap()
            .window_minutes,
        300
    );
    assert_eq!(
        batch.observations[0]
            .weekly
            .as_ref()
            .unwrap()
            .window_minutes,
        10_080
    );
    assert_eq!(batch.observations[0].plan_type.as_deref(), Some("plus"));
}

#[test]
fn detects_moved_windows_and_retains_metadata_reset_and_schema() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("moved.jsonl");
    fs::write(
        &path,
        concat!(
            "{\"timestamp\":\"2030-01-01T20:00:00+08:00\",\"type\":\"event_msg\",",
            "\"model\":\"gpt-test\",\"payload\":{\"type\":\"token_count\",",
            "\"service_tier\":\"fast\",\"rate_limits\":{\"added_field\":true,",
            "\"primary\":{\"used_percent\":56,\"window_minutes\":10080,",
            "\"resets_at\":\"2030-01-08T08:00:00+08:00\"},",
            "\"secondary\":{\"used_percent\":42,\"window_minutes\":300,",
            "\"resets_at\":1893502800},\"plan_type\":\"plus\"}}}\n"
        ),
    )
    .unwrap();

    let batch = ingest_transcript(&path, TranscriptCursor::default(), &fixture_options()).unwrap();
    let observation = &batch.observations[0];
    assert_eq!(observation.observed_at, dt("2030-01-01T12:00:00Z"));
    assert_eq!(observation.model_slug.as_deref(), Some("gpt-test"));
    assert_eq!(observation.service_tier.as_deref(), Some("fast"));
    assert_eq!(observation.five_hour.as_ref().unwrap().used_percent, 42.0);
    assert_eq!(observation.weekly.as_ref().unwrap().used_percent, 56.0);
    assert_eq!(
        observation.weekly.as_ref().unwrap().resets_at,
        Some(dt("2030-01-08T00:00:00Z"))
    );
    assert!(observation.schema_fingerprint.contains("added_field:bool"));
}

#[test]
fn unsupported_and_missing_shapes_are_diagnostics_never_zero_readings() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("unsupported.jsonl");
    fs::write(
        &path,
        concat!(
            "{\"timestamp\":\"2030-01-01T12:00:00Z\",\"type\":\"event_msg\",",
            "\"payload\":{\"type\":\"token_count\",\"rate_limits\":{",
            "\"primary\":{\"used_percent\":\"0\",\"window_minutes\":10080},",
            "\"secondary\":{\"used_percent\":4,\"window_minutes\":60}}}}\n",
            "{\"timestamp\":\"2030-01-01T12:01:00Z\",\"type\":\"event_msg\",",
            "\"payload\":{\"type\":\"token_count\"}}\n"
        ),
    )
    .unwrap();

    let batch = ingest_transcript(&path, TranscriptCursor::default(), &fixture_options()).unwrap();
    assert!(batch.snapshots.is_empty());
    assert_eq!(batch.observations.len(), 1);
    assert!(batch.observations[0].weekly.is_none());
    let codes: Vec<_> = batch
        .diagnostics
        .iter()
        .map(|item| item.code.as_str())
        .collect();
    assert!(codes.contains(&"unsupported_window_shape"));
    assert!(codes.contains(&"unsupported_window_duration"));
    assert!(codes.contains(&"missing_weekly_window"));
    assert!(codes.contains(&"missing_rate_limits"));
    assert!(
        !batch
            .diagnostics
            .iter()
            .any(|item| item.message.contains("prompt"))
    );
}

#[test]
fn malformed_final_line_is_deferred_without_losing_valid_prefix() {
    let path = fixture("malformed_final_line.jsonl");
    let first = ingest_transcript(&path, TranscriptCursor::default(), &fixture_options()).unwrap();

    assert_eq!(
        first
            .snapshots
            .iter()
            .map(|snapshot| snapshot.used_percent)
            .collect::<Vec<_>>(),
        vec![20.0, 21.0]
    );
    assert!(first.incomplete_tail);
    assert_eq!(first.skipped_malformed_lines, 0);

    let retry = ingest_transcript(
        &path,
        TranscriptCursor {
            next_offset: first.next_offset,
        },
        &fixture_options(),
    )
    .unwrap();
    assert!(retry.snapshots.is_empty());
    assert!(retry.incomplete_tail);
    assert_eq!(retry.next_offset, first.next_offset);
}

#[test]
fn cursor_reads_only_newly_appended_records() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("session.jsonl");
    let first_line = include_str!("fixtures/normal_growth.jsonl")
        .lines()
        .next()
        .unwrap();
    fs::write(&path, format!("{first_line}\n")).unwrap();

    let first = ingest_transcript(&path, TranscriptCursor::default(), &fixture_options()).unwrap();
    assert_eq!(first.snapshots.len(), 1);

    let second_line = include_str!("fixtures/normal_growth.jsonl")
        .lines()
        .nth(1)
        .unwrap();
    fs::write(&path, format!("{first_line}\n{second_line}\n")).unwrap();
    let second = ingest_transcript(
        &path,
        TranscriptCursor {
            next_offset: first.next_offset,
        },
        &fixture_options(),
    )
    .unwrap();
    assert_eq!(second.snapshots.len(), 1);
    assert_eq!(second.snapshots[0].used_percent, 22.0);
}

#[test]
fn cursor_beyond_a_truncated_transcript_restarts_safely() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("truncated.jsonl");
    let line = include_str!("fixtures/normal_growth.jsonl")
        .lines()
        .next()
        .unwrap();
    fs::write(&path, format!("{line}\n")).unwrap();

    let batch = ingest_transcript(
        &path,
        TranscriptCursor {
            next_offset: 10_000,
        },
        &fixture_options(),
    )
    .unwrap();
    assert_eq!(batch.start_offset, 0);
    assert_eq!(batch.snapshots.len(), 1);
}

#[test]
fn hook_input_uses_its_transcript_path() {
    let input = HookInput {
        transcript_path: fixture("normal_growth.jsonl"),
    };
    let batch =
        ingest_hook_transcript(&input, TranscriptCursor::default(), &fixture_options()).unwrap();
    assert_eq!(batch.snapshots.len(), 2);
}

#[test]
fn irrelevant_and_invalid_records_are_skipped_without_exposing_content() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("mixed.jsonl");
    let content = concat!(
        "{\"timestamp\":\"2030-01-01T12:00:00Z\",\"type\":\"response_item\",\"payload\":{}}\n",
        "{\"timestamp\":\"bad\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"rate_limits\":{\"primary\":{\"used_percent\":3,\"window_minutes\":10080}}}}\n",
        "{not json}\n",
        "{\"timestamp\":\"2030-01-01T12:01:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"rate_limits\":{\"primary\":{\"used_percent\":4,\"window_minutes\":10080}}}}\n"
    );
    fs::write(&path, content).unwrap();

    let batch = ingest_transcript(&path, TranscriptCursor::default(), &fixture_options()).unwrap();
    assert_eq!(batch.snapshots.len(), 1);
    assert_eq!(batch.snapshots[0].used_percent, 4.0);
    assert_eq!(batch.skipped_irrelevant_records, 1);
    assert_eq!(batch.skipped_invalid_records, 1);
    assert_eq!(batch.skipped_malformed_lines, 1);
}

#[test]
fn future_records_beyond_tolerance_are_invalid() {
    let batch = ingest_transcript(
        &fixture("normal_growth.jsonl"),
        TranscriptCursor::default(),
        &IngestOptions {
            now: dt("2029-12-01T00:00:00Z"),
            future_tolerance: TimeDelta::minutes(5),
        },
    )
    .unwrap();
    assert!(batch.snapshots.is_empty());
    assert_eq!(batch.skipped_invalid_records, 2);
}

#[test]
fn discovery_is_bounded_by_days_entries_and_result_count() {
    let temp = TempDir::new().unwrap();
    let recent = temp.path().join("2030/01/03");
    let old = temp.path().join("2029/12/01");
    fs::create_dir_all(&recent).unwrap();
    fs::create_dir_all(&old).unwrap();
    for name in ["a.jsonl", "b.jsonl", "c.txt"] {
        fs::write(recent.join(name), "").unwrap();
    }
    fs::write(old.join("old.jsonl"), "").unwrap();

    let found = discover_recent_transcripts(
        temp.path(),
        dt("2030-01-03T12:00:00Z"),
        DiscoveryOptions {
            lookback_days: 2,
            max_files: 1,
            max_entries_per_day: 10,
        },
    )
    .unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].extension().unwrap(), "jsonl");
    assert!(!found[0].starts_with(old));
}

#[test]
fn session_metadata_partitions_imported_evidence_without_retaining_content() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("session.jsonl");
    let content = concat!(
        "{\"type\":\"session_meta\",\"payload\":{\"cli_version\":\"0.146.0\",\"model\":\"gpt-context\",\"service_tier\":\"fast\",\"plan_type\":\"plus\",\"instructions\":\"PRIVATE\"}}\n",
        "{\"timestamp\":\"2030-01-01T12:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"rate_limits\":{\"primary\":{\"used_percent\":42,\"window_minutes\":300,\"resets_at\":1893502800},\"secondary\":{\"used_percent\":56,\"window_minutes\":10080,\"resets_at\":1893974400}}}}\n"
    );
    fs::write(&path, content).unwrap();
    let batch = ingest_transcript(&path, TranscriptCursor::default(), &fixture_options()).unwrap();
    assert_eq!(batch.observations.len(), 1);
    let observation = &batch.observations[0];
    assert_eq!(observation.codex_version.as_deref(), Some("0.146.0"));
    assert_eq!(observation.model_slug.as_deref(), Some("gpt-context"));
    assert_eq!(observation.service_tier.as_deref(), Some("fast"));
    assert_eq!(observation.plan_type.as_deref(), Some("plus"));
    assert!(
        !serde_json::to_string(observation)
            .unwrap()
            .contains("PRIVATE")
    );
}
