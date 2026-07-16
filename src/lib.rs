//! Accounting, transcript ingestion, and durable state for Codex Usage Watch.
//!
//! SQLite is authoritative; `display.json` is a replaceable projection. The
//! Stage 5 hook adapter is exposed for the installed `codex-5h hook` command.

pub mod calculate;
pub mod calibration;
pub mod compatibility;
pub mod hooks;
pub mod ingest;
pub mod model;
pub mod persistence;
pub mod private_fs;
pub mod setup;

pub use calculate::{AccountingEngine, ApplyOutcome, round_five_hour_percent, round_weekly_points};
pub use calibration::{
    CalibrationConfidence, CalibrationIdentity, CalibrationProfile, CalibrationReport,
    CalibrationSample, EvidenceQuality, GroundTruthStatus, MINIMUM_RECOMMENDATION_SAMPLES,
};
pub use compatibility::{
    CompatibilityCheck, CompatibilityIdentity, CompatibilityResult, ReleaseMetadata,
    cached_release_metadata,
};
pub use ingest::{
    DiscoveryOptions, HookInput, IngestBatch, IngestOptions, MAX_JSONL_RECORD_BYTES,
    TranscriptCursor, discover_recent_transcripts, ingest_hook_transcript, ingest_transcript,
};
pub use model::{
    DomainError, FIVE_HOUR_WINDOW_MINUTES, IngestDiagnostic, LocalWindow, MeterReading,
    ObservationId, ObservedRateLimitWindow, TrackerConfig, UsageObservation, WEEKLY_WINDOW_MINUTES,
    WeeklySnapshot, WindowStatus,
};
pub use persistence::{
    CalibrationKind, ControlEvent, DisplayCacheV1, PersistOutcome, PersistTranscriptOutcome,
    StateError, StatePaths, StateStore, WindowHistoryEntry,
};
pub use setup::{HistoryImportSummary, HistoryPreview, import_history, preview_history};
