use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, TimeDelta, Utc};
use codex_usage_watch::{
    DiscoveryOptions, HookInput, IngestOptions, MAX_JSONL_RECORD_BYTES, TranscriptCursor,
    discover_recent_transcripts, ingest_hook_transcript, ingest_transcript,
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

#[test]
fn oversized_records_are_bounded_and_do_not_hide_later_valid_records() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("oversized.jsonl");
    let valid = include_str!("fixtures/normal_growth.jsonl")
        .lines()
        .next()
        .unwrap();
    let mut contents = vec![b' '; MAX_JSONL_RECORD_BYTES + 64];
    contents.push(b'\n');
    contents.extend_from_slice(valid.as_bytes());
    contents.push(b'\n');
    fs::write(&path, contents).unwrap();

    let batch = ingest_transcript(&path, TranscriptCursor::default(), &fixture_options()).unwrap();
    assert_eq!(batch.skipped_oversized_records, 1);
    assert_eq!(batch.snapshots.len(), 1);
    assert_eq!(batch.snapshots[0].used_percent, 20.0);
    assert!(batch.diagnostics.iter().any(|item| {
        item.code == "oversized_record" && !item.message.contains(path.to_string_lossy().as_ref())
    }));
    assert_eq!(batch.next_offset, fs::metadata(path).unwrap().len());
}

#[cfg(unix)]
#[test]
fn transcript_paths_with_spaces_and_quotes_are_supported() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("session space 'quote'.jsonl");
    fs::copy(fixture("normal_growth.jsonl"), &path).unwrap();
    let batch = ingest_transcript(&path, TranscriptCursor::default(), &fixture_options()).unwrap();
    assert_eq!(batch.snapshots.len(), 2);
    assert_eq!(batch.source_file, fs::canonicalize(path).unwrap());
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn transcript_paths_with_non_utf8_bytes_are_supported_where_the_filesystem_allows_them() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let temp = TempDir::new().unwrap();
    let mut name = b"session-non-utf8-".to_vec();
    name.push(0xff);
    name.extend_from_slice(b".jsonl");
    let path = temp.path().join(OsString::from_vec(name));
    fs::copy(fixture("normal_growth.jsonl"), &path).unwrap();
    let batch = ingest_transcript(&path, TranscriptCursor::default(), &fixture_options()).unwrap();
    assert_eq!(batch.snapshots.len(), 2);
    assert_eq!(batch.source_file, fs::canonicalize(path).unwrap());
}

#[test]
fn discovered_file_can_be_replaced_before_ingestion() {
    let temp = TempDir::new().unwrap();
    let day = temp.path().join("2030/01/03");
    fs::create_dir_all(&day).unwrap();
    let path = day.join("replace-me.jsonl");
    fs::write(&path, b"old\n").unwrap();
    let found = discover_recent_transcripts(
        temp.path(),
        dt("2030-01-03T12:00:00Z"),
        DiscoveryOptions::default(),
    )
    .unwrap();
    fs::copy(fixture("normal_growth.jsonl"), &path).unwrap();
    let batch =
        ingest_transcript(&found[0], TranscriptCursor::default(), &fixture_options()).unwrap();
    assert_eq!(batch.snapshots.len(), 2);
}

#[test]
fn large_history_discovery_stays_inside_configured_bounds() {
    let temp = TempDir::new().unwrap();
    let day = temp.path().join("2030/01/03");
    fs::create_dir_all(&day).unwrap();
    for index in 0..400 {
        fs::write(day.join(format!("session-{index:03}.jsonl")), b"{}\n").unwrap();
    }
    let found = discover_recent_transcripts(
        temp.path(),
        dt("2030-01-03T12:00:00Z"),
        DiscoveryOptions {
            lookback_days: 1,
            max_files: 8,
            max_entries_per_day: 64,
        },
    )
    .unwrap();
    assert_eq!(found.len(), 8);
}

#[test]
fn discovery_sorts_every_candidate_before_applying_the_daily_bound() {
    let temp = TempDir::new().unwrap();
    let day = temp.path().join("2030/01/03");
    fs::create_dir_all(&day).unwrap();
    let old_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_893_456_000);
    for index in 0..300 {
        let path = day.join(format!("older-{index:03}.jsonl"));
        fs::write(&path, b"{}\n").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old_time)
            .unwrap();
    }
    let newest = day.join("newest-usable.jsonl");
    fs::copy(fixture("normal_growth.jsonl"), &newest).unwrap();
    fs::File::options()
        .write(true)
        .open(&newest)
        .unwrap()
        .set_modified(old_time + Duration::from_secs(60))
        .unwrap();

    let options = DiscoveryOptions {
        lookback_days: 1,
        max_files: 8,
        max_entries_per_day: 256,
    };
    let first =
        discover_recent_transcripts(temp.path(), dt("2030-01-03T12:00:00Z"), options).unwrap();
    let second =
        discover_recent_transcripts(temp.path(), dt("2030-01-03T12:00:00Z"), options).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.first(), Some(&newest));
    let batch =
        ingest_transcript(&first[0], TranscriptCursor::default(), &fixture_options()).unwrap();
    assert_eq!(batch.snapshots.len(), 2);
}

#[test]
fn discovery_uses_path_order_to_break_equal_timestamp_ties() {
    let temp = TempDir::new().unwrap();
    let day = temp.path().join("2030/01/03");
    fs::create_dir_all(&day).unwrap();
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_893_456_000);
    for name in ["c.jsonl", "a.jsonl", "b.jsonl"] {
        let path = day.join(name);
        fs::write(&path, b"{}\n").unwrap();
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    let found = discover_recent_transcripts(
        temp.path(),
        dt("2030-01-03T12:00:00Z"),
        DiscoveryOptions {
            lookback_days: 1,
            max_files: 3,
            max_entries_per_day: 3,
        },
    )
    .unwrap();
    let names: Vec<_> = found
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["a.jsonl", "b.jsonl", "c.jsonl"]);
}
