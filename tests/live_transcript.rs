use std::path::Path;

use codex_usage_watch::{IngestOptions, TranscriptCursor, ingest_transcript};

/// Local acceptance check for Stage 2.
///
/// Run with both variables set. The expected value must come from a separate
/// manual inspection (for example, a narrow `jq` query of rate-limit fields).
#[test]
#[ignore = "requires a real local Codex transcript and manually inspected expected value"]
fn latest_weekly_value_matches_manual_inspection() {
    let path = std::env::var("CODEX_USAGE_WATCH_REAL_LOG").unwrap();
    let expected: f64 = std::env::var("CODEX_USAGE_WATCH_EXPECTED_WEEKLY")
        .unwrap()
        .parse()
        .unwrap();
    let batch = ingest_transcript(
        Path::new(&path),
        TranscriptCursor::default(),
        &IngestOptions::default(),
    )
    .unwrap();
    let latest = batch
        .snapshots
        .iter()
        .max_by_key(|snapshot| (&snapshot.observed_at, &snapshot.id))
        .expect("the selected transcript should contain a weekly snapshot");
    assert_eq!(latest.used_percent, expected);
}
