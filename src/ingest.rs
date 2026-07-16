use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Days, TimeDelta, Utc};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use crate::model::{
    FIVE_HOUR_WINDOW_MINUTES, IngestDiagnostic, ObservationId, ObservedRateLimitWindow,
    UsageObservation, WEEKLY_WINDOW_MINUTES, WeeklySnapshot,
};

/// A single transcript record is bounded so a malformed local file cannot make
/// a lifecycle hook allocate memory proportional to an attacker-controlled line.
pub const MAX_JSONL_RECORD_BYTES: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum IngestError {
    #[error("could not access transcript {path}: {source}")]
    TranscriptIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("discovery options must have non-zero bounds")]
    InvalidDiscoveryBounds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TranscriptCursor {
    pub next_offset: u64,
}

#[derive(Debug, Clone)]
pub struct IngestOptions {
    pub now: DateTime<Utc>,
    pub future_tolerance: TimeDelta,
}

impl Default for IngestOptions {
    fn default() -> Self {
        Self {
            now: Utc::now(),
            future_tolerance: TimeDelta::minutes(5),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IngestBatch {
    pub snapshots: Vec<WeeklySnapshot>,
    pub observations: Vec<UsageObservation>,
    pub diagnostics: Vec<IngestDiagnostic>,
    pub source_file: PathBuf,
    pub start_offset: u64,
    pub next_offset: u64,
    pub incomplete_tail: bool,
    pub skipped_malformed_lines: usize,
    pub skipped_oversized_records: usize,
    pub skipped_invalid_records: usize,
    pub skipped_irrelevant_records: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct HookInput {
    pub transcript_path: PathBuf,
}

pub fn ingest_hook_transcript(
    input: &HookInput,
    cursor: TranscriptCursor,
    options: &IngestOptions,
) -> Result<IngestBatch, IngestError> {
    ingest_transcript(&input.transcript_path, cursor, options)
}

pub fn ingest_transcript(
    path: &Path,
    cursor: TranscriptCursor,
    options: &IngestOptions,
) -> Result<IngestBatch, IngestError> {
    let canonical_path = fs::canonicalize(path).map_err(|source| IngestError::TranscriptIo {
        path: path.to_path_buf(),
        source,
    })?;
    let mut file = File::open(&canonical_path).map_err(|source| IngestError::TranscriptIo {
        path: canonical_path.clone(),
        source,
    })?;
    let file_len = file
        .metadata()
        .map_err(|source| IngestError::TranscriptIo {
            path: canonical_path.clone(),
            source,
        })?
        .len();
    // A cursor beyond EOF means the transcript was truncated or replaced.
    // Restarting at zero is safe because persisted observations are idempotent
    // by source path and byte offset.
    let start_offset = if cursor.next_offset > file_len {
        0
    } else {
        cursor.next_offset
    };
    file.seek(SeekFrom::Start(start_offset))
        .map_err(|source| IngestError::TranscriptIo {
            path: canonical_path.clone(),
            source,
        })?;

    let mut reader = BufReader::new(file);
    let mut next_offset = start_offset;
    let mut batch = IngestBatch {
        snapshots: Vec::new(),
        observations: Vec::new(),
        diagnostics: Vec::new(),
        source_file: canonical_path.clone(),
        start_offset,
        next_offset,
        incomplete_tail: false,
        skipped_malformed_lines: 0,
        skipped_oversized_records: 0,
        skipped_invalid_records: 0,
        skipped_irrelevant_records: 0,
    };

    let mut context = TranscriptContext::default();
    loop {
        let line_offset = next_offset;
        let record =
            read_bounded_record(&mut reader).map_err(|source| IngestError::TranscriptIo {
                path: canonical_path.clone(),
                source,
            })?;
        let bytes_read = record.bytes_read;
        if bytes_read == 0 {
            break;
        }

        let line_end = line_offset + bytes_read as u64;
        let is_final_line = line_end == file_len;
        if record.oversized {
            batch.diagnostics.push(diagnostic(
                &ObservationId::new(&canonical_path, line_offset),
                None,
                "oversized_record",
                Some("record"),
                "JSONL record exceeds the 1048576-byte safety limit",
            ));
            batch.skipped_oversized_records += 1;
            batch.skipped_invalid_records += 1;
            next_offset = line_end;
            batch.next_offset = next_offset;
            continue;
        }
        let parsed = serde_json::from_slice::<Value>(
            record.bytes.strip_suffix(b"\r").unwrap_or(&record.bytes),
        );
        let value = match parsed {
            Ok(value) => value,
            Err(_) if is_final_line => {
                batch.incomplete_tail = true;
                batch.next_offset = line_offset;
                break;
            }
            Err(_) => {
                batch.skipped_malformed_lines += 1;
                next_offset = line_end;
                batch.next_offset = next_offset;
                continue;
            }
        };

        next_offset = line_end;
        batch.next_offset = next_offset;

        if update_transcript_context(&value, &mut context) {
            batch.skipped_irrelevant_records += 1;
            continue;
        }
        match parse_record(value, &canonical_path, line_offset, options, &context) {
            ParsedRecord::Relevant(relevant) => {
                let ParsedRelevant {
                    observation,
                    snapshot,
                    diagnostics,
                    invalid,
                } = *relevant;
                if let Some(observation) = observation {
                    batch.observations.push(observation);
                }
                if let Some(snapshot) = snapshot {
                    batch.snapshots.push(snapshot);
                }
                batch.diagnostics.extend(diagnostics);
                if invalid {
                    batch.skipped_invalid_records += 1;
                }
            }
            ParsedRecord::Irrelevant => batch.skipped_irrelevant_records += 1,
        }
    }

    Ok(batch)
}

struct BoundedRecord {
    bytes: Vec<u8>,
    bytes_read: usize,
    oversized: bool,
}

fn read_bounded_record(reader: &mut impl BufRead) -> io::Result<BoundedRecord> {
    let mut bytes = Vec::new();
    let mut bytes_read = 0usize;
    let mut oversized = false;

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        let content_len = newline.unwrap_or(consumed);
        if !oversized {
            let remaining = MAX_JSONL_RECORD_BYTES.saturating_sub(bytes.len());
            let retained = content_len.min(remaining);
            bytes.extend_from_slice(&available[..retained]);
            if content_len > remaining {
                oversized = true;
            }
        }
        bytes_read = bytes_read.saturating_add(consumed);
        reader.consume(consumed);
        if newline.is_some() {
            break;
        }
    }

    Ok(BoundedRecord {
        bytes,
        bytes_read,
        oversized,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryOptions {
    pub lookback_days: u32,
    pub max_files: usize,
    pub max_entries_per_day: usize,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            lookback_days: 14,
            max_files: 64,
            max_entries_per_day: 256,
        }
    }
}

pub fn discover_recent_transcripts(
    sessions_root: &Path,
    now: DateTime<Utc>,
    options: DiscoveryOptions,
) -> Result<Vec<PathBuf>, IngestError> {
    if options.lookback_days == 0 || options.max_files == 0 || options.max_entries_per_day == 0 {
        return Err(IngestError::InvalidDiscoveryBounds);
    }

    let mut candidates: Vec<(SystemTime, PathBuf)> = Vec::new();
    for day_offset in 0..options.lookback_days {
        let Some(date) = now
            .date_naive()
            .checked_sub_days(Days::new(u64::from(day_offset)))
        else {
            break;
        };
        let day_dir = sessions_root
            .join(date.format("%Y").to_string())
            .join(date.format("%m").to_string())
            .join(date.format("%d").to_string());
        let entries = match fs::read_dir(&day_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(IngestError::TranscriptIo {
                    path: day_dir,
                    source,
                });
            }
        };

        for entry in entries.take(options.max_entries_per_day) {
            let entry = entry.map_err(|source| IngestError::TranscriptIo {
                path: day_dir.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            candidates.push((modified, path));
        }
    }

    candidates.sort_by(|left, right| right.cmp(left));
    candidates.truncate(options.max_files);
    Ok(candidates.into_iter().map(|(_, path)| path).collect())
}

enum ParsedRecord {
    Relevant(Box<ParsedRelevant>),
    Irrelevant,
}

struct ParsedRelevant {
    observation: Option<UsageObservation>,
    snapshot: Option<WeeklySnapshot>,
    diagnostics: Vec<IngestDiagnostic>,
    invalid: bool,
}

#[derive(Default)]
struct TranscriptContext {
    codex_version: Option<String>,
    model_slug: Option<String>,
    service_tier: Option<String>,
    plan_type: Option<String>,
}

fn update_transcript_context(value: &Value, context: &mut TranscriptContext) -> bool {
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return false;
    }
    let payload = value.get("payload").unwrap_or(value);
    context.codex_version = string_metadata(payload, "cli_version")
        .or_else(|| string_metadata(payload, "codex_version"))
        .or_else(|| context.codex_version.clone());
    context.model_slug = string_metadata(payload, "model")
        .or_else(|| string_metadata(payload, "model_slug"))
        .or_else(|| context.model_slug.clone());
    context.service_tier =
        string_metadata(payload, "service_tier").or_else(|| context.service_tier.clone());
    context.plan_type = string_metadata(payload, "plan_type").or_else(|| context.plan_type.clone());
    true
}

fn parse_record(
    envelope: Value,
    source_path: &Path,
    byte_offset: u64,
    options: &IngestOptions,
    context: &TranscriptContext,
) -> ParsedRecord {
    if envelope.get("type").and_then(Value::as_str) != Some("event_msg") {
        return ParsedRecord::Irrelevant;
    }
    let Some(payload) = envelope.get("payload") else {
        return invalid_record(
            source_path,
            byte_offset,
            None,
            "missing_payload",
            "payload",
            "token event payload is missing",
        );
    };
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return ParsedRecord::Irrelevant;
    }
    let id = ObservationId::new(source_path, byte_offset);
    let Some(timestamp) = envelope.get("timestamp").and_then(Value::as_str) else {
        return invalid_record(
            source_path,
            byte_offset,
            None,
            "invalid_timestamp",
            "timestamp",
            "timestamp must be an RFC3339 string",
        );
    };
    let Ok(observed_at) =
        DateTime::parse_from_rfc3339(timestamp).map(|value| value.with_timezone(&Utc))
    else {
        return invalid_record(
            source_path,
            byte_offset,
            None,
            "invalid_timestamp",
            "timestamp",
            "timestamp must be an RFC3339 string",
        );
    };
    if observed_at > options.now + options.future_tolerance {
        return invalid_record(
            source_path,
            byte_offset,
            Some(observed_at),
            "future_timestamp",
            "timestamp",
            "timestamp is beyond the allowed future skew",
        );
    }

    let Some(rate_limits) = payload.get("rate_limits") else {
        return invalid_record(
            source_path,
            byte_offset,
            Some(observed_at),
            "missing_rate_limits",
            "payload.rate_limits",
            "token event has no rate-limit object",
        );
    };
    let schema_fingerprint = schema_fingerprint(rate_limits);
    let Some(rate_limits_object) = rate_limits.as_object() else {
        return invalid_record(
            source_path,
            byte_offset,
            Some(observed_at),
            "unsupported_rate_limits_type",
            "payload.rate_limits",
            "rate_limits must be an object",
        );
    };

    let mut diagnostics = Vec::new();
    let mut five_hour = None;
    let mut weekly = None;
    for slot in ["primary", "secondary"] {
        let Some(value) = rate_limits_object.get(slot) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        match parse_window(value) {
            Ok(window) if window.window_minutes == FIVE_HOUR_WINDOW_MINUTES => {
                if five_hour.replace(window).is_some() {
                    diagnostics.push(diagnostic(
                        &id,
                        Some(observed_at),
                        "duplicate_five_hour_window",
                        Some(slot),
                        "multiple 300-minute windows were present",
                    ));
                }
            }
            Ok(window) if window.window_minutes == WEEKLY_WINDOW_MINUTES => {
                if weekly.replace(window).is_some() {
                    diagnostics.push(diagnostic(
                        &id,
                        Some(observed_at),
                        "duplicate_weekly_window",
                        Some(slot),
                        "multiple 10080-minute windows were present",
                    ));
                }
            }
            Ok(_) => diagnostics.push(diagnostic(
                &id,
                Some(observed_at),
                "unsupported_window_duration",
                Some(slot),
                "window duration is neither 300 nor 10080 minutes",
            )),
            Err(message) => diagnostics.push(diagnostic(
                &id,
                Some(observed_at),
                "unsupported_window_shape",
                Some(slot),
                message,
            )),
        }
    }
    if five_hour.is_none() {
        diagnostics.push(diagnostic(
            &id,
            Some(observed_at),
            "missing_five_hour_window",
            None,
            "no valid 300-minute window was present",
        ));
    }
    if weekly.is_none() {
        diagnostics.push(diagnostic(
            &id,
            Some(observed_at),
            "missing_weekly_window",
            None,
            "no valid 10080-minute window was present",
        ));
    }

    let plan_type = string_metadata(rate_limits, "plan_type").or_else(|| context.plan_type.clone());
    let model_slug = string_metadata(payload, "model")
        .or_else(|| string_metadata(payload, "model_slug"))
        .or_else(|| string_metadata(&envelope, "model"));
    let model_slug = model_slug.or_else(|| context.model_slug.clone());
    let service_tier = string_metadata(payload, "service_tier")
        .or_else(|| string_metadata(rate_limits, "service_tier"))
        .or_else(|| string_metadata(&envelope, "service_tier"));
    let service_tier = service_tier.or_else(|| context.service_tier.clone());
    let snapshot = weekly.as_ref().and_then(|window| {
        WeeklySnapshot::new(
            id.clone(),
            observed_at,
            window.used_percent,
            window.resets_at,
            window.window_minutes,
            plan_type.clone(),
        )
        .ok()
    });
    let invalid = weekly.is_none()
        || diagnostics
            .iter()
            .any(|item| item.code.starts_with("unsupported_"));
    let observation = UsageObservation {
        id,
        observed_at,
        five_hour,
        weekly,
        model_slug,
        codex_version: context.codex_version.clone(),
        service_tier,
        plan_type,
        schema_fingerprint,
    };

    ParsedRecord::Relevant(Box::new(ParsedRelevant {
        observation: Some(observation),
        snapshot,
        diagnostics,
        invalid,
    }))
}

fn parse_window(value: &Value) -> Result<ObservedRateLimitWindow, &'static str> {
    let Some(object) = value.as_object() else {
        return Err("window must be an object");
    };
    let Some(used_percent) = object.get("used_percent").and_then(Value::as_f64) else {
        return Err("used_percent must be a finite number");
    };
    if !used_percent.is_finite() || !(0.0..=100.0).contains(&used_percent) {
        return Err("used_percent must be between 0 and 100");
    }
    let Some(window_minutes) = object
        .get("window_minutes")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
    else {
        return Err("window_minutes must be an unsigned integer");
    };
    let resets_at = match object.get("resets_at") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => number
            .as_i64()
            .and_then(|value| DateTime::from_timestamp(value, 0))
            .ok_or("resets_at must be a valid Unix timestamp")
            .map(Some)?,
        Some(Value::String(value)) => DateTime::parse_from_rfc3339(value)
            .map(|value| Some(value.with_timezone(&Utc)))
            .map_err(|_| "resets_at must be a Unix timestamp or RFC3339 string")?,
        Some(_) => return Err("resets_at must be a Unix timestamp, RFC3339 string, or null"),
    };
    Ok(ObservedRateLimitWindow {
        used_percent,
        window_minutes,
        resets_at,
    })
}

fn invalid_record(
    source_path: &Path,
    byte_offset: u64,
    observed_at: Option<DateTime<Utc>>,
    code: &str,
    field: &str,
    message: &str,
) -> ParsedRecord {
    let id = ObservationId::new(source_path, byte_offset);
    ParsedRecord::Relevant(Box::new(ParsedRelevant {
        observation: None,
        snapshot: None,
        diagnostics: vec![diagnostic(&id, observed_at, code, Some(field), message)],
        invalid: true,
    }))
}

fn diagnostic(
    id: &ObservationId,
    observed_at: Option<DateTime<Utc>>,
    code: &str,
    field: Option<&str>,
    message: &str,
) -> IngestDiagnostic {
    IngestDiagnostic {
        id: id.clone(),
        observed_at,
        code: code.to_owned(),
        field: field.map(str::to_owned),
        message: message.to_owned(),
    }
}

fn string_metadata(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn schema_fingerprint(rate_limits: &Value) -> String {
    fn visit(value: &Value, path: &str, parts: &mut Vec<String>) {
        match value {
            Value::Null => parts.push(format!("{path}:null")),
            Value::Bool(_) => parts.push(format!("{path}:bool")),
            Value::Number(number) => {
                if path.ends_with("window_minutes") {
                    parts.push(format!("{path}:number={number}"));
                } else {
                    parts.push(format!("{path}:number"));
                }
            }
            Value::String(_) => parts.push(format!("{path}:string")),
            Value::Array(_) => parts.push(format!("{path}:array")),
            Value::Object(object) => {
                parts.push(format!("{path}:object"));
                let mut keys: Vec<_> = object.keys().collect();
                keys.sort_unstable();
                for key in keys {
                    visit(&object[key], &format!("{path}.{key}"), parts);
                }
            }
        }
    }

    let mut parts = Vec::new();
    visit(rate_limits, "rate_limits", &mut parts);
    parts.join("|")
}
