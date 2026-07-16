#![no_main]

use std::io::Write;

use chrono::{TimeDelta, Utc};
use codex_usage_watch::{IngestOptions, TranscriptCursor, ingest_transcript};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut transcript = tempfile::NamedTempFile::new().expect("temporary transcript");
    transcript.write_all(data).expect("write fuzz input");
    let _ = ingest_transcript(
        transcript.path(),
        TranscriptCursor::default(),
        &IngestOptions {
            now: Utc::now(),
            future_tolerance: TimeDelta::minutes(5),
        },
    );
});
