use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::calibration::{
    CalibrationConfidence, CalibrationIdentity, CalibrationObservation, CalibrationProfile,
    CalibrationReport, build_report, stable_calibration_id,
};
use crate::compatibility::{CompatibilityCheck, CompatibilityIdentity, CompatibilityResult};
use crate::ingest::{IngestBatch, IngestError, IngestOptions, TranscriptCursor, ingest_transcript};
use crate::model::{
    DomainError, IngestDiagnostic, ObservedRateLimitWindow, TrackerConfig, UsageObservation,
    WeeklySnapshot, WindowStatus,
};

const SCHEMA_VERSION: i64 = 10;
const DISPLAY_SCHEMA_VERSION: u32 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Error)]
pub enum StateError {
    #[error("could not determine a per-user application data directory")]
    StateDirectoryUnavailable,
    #[error("state I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("SQLite state failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("state contains an invalid timestamp: {0}")]
    InvalidTimestamp(String),
    #[error("state contains invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("transcript ingestion failed: {0}")]
    Ingest(#[from] IngestError),
    #[error("database schema version {found} is newer than supported version {supported}")]
    UnsupportedSchemaVersion { found: i64, supported: i64 },
    #[error("invalid tracker configuration: {0}")]
    Domain(#[from] DomainError),
    #[error("no supported exact-identity calibration is available to approve")]
    UnsupportedCalibrationIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatePaths {
    pub directory: PathBuf,
    pub database: PathBuf,
    pub display: PathBuf,
    pub calibration_report: PathBuf,
}

impl StatePaths {
    pub fn resolve(config: &TrackerConfig) -> Result<Self, StateError> {
        let directory = if let Some(path) = config.state_dir_override() {
            path.to_path_buf()
        } else if let Some(path) = std::env::var_os("CODEX_USAGE_WATCH_HOME") {
            PathBuf::from(path)
        } else {
            ProjectDirs::from("dev", "codex-usage-watch", "codex-usage-watch")
                .map(|dirs| dirs.data_local_dir().to_path_buf())
                .ok_or(StateError::StateDirectoryUnavailable)?
        };
        Ok(Self::in_directory(directory))
    }

    pub fn in_directory(directory: impl Into<PathBuf>) -> Self {
        let directory = directory.into();
        Self {
            database: directory.join("state.sqlite3"),
            display: directory.join("display.json"),
            calibration_report: directory.join("calibration-report.json"),
            directory,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationKind {
    Historical,
    Measured,
    InheritedUnvalidated,
}

impl CalibrationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Historical => "historical",
            Self::Measured => "measured",
            Self::InheritedUnvalidated => "inherited_unvalidated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayCacheV1 {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub status: WindowStatus,
    pub stale: bool,
    pub data_age_seconds: Option<i64>,
    pub observed_at: Option<DateTime<Utc>>,
    pub window_started_at: Option<DateTime<Utc>>,
    pub window_ends_at: Option<DateTime<Utc>>,
    pub weekly_points: Option<f64>,
    pub five_hour_estimate_percent: Option<f64>,
    pub five_hour_estimate_left_percent: Option<f64>,
    #[serde(default)]
    pub five_hour_value_source: Option<String>,
    pub weekly_limit_used_percent: Option<f64>,
    pub weekly_limit_left_percent: Option<f64>,
    pub calibration_weekly_points: Option<f64>,
    pub calibration_kind: CalibrationKind,
    #[serde(default)]
    pub calibration_id: Option<String>,
    #[serde(default)]
    pub calibration_confidence: Option<CalibrationConfidence>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersistOutcome {
    pub inserted_snapshots: usize,
    pub newly_emitted_warnings: Vec<u32>,
    pub display: DisplayCacheV1,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersistTranscriptOutcome {
    pub batch: IngestBatch,
    pub inserted_observations: usize,
    pub inserted_diagnostics: usize,
    pub persisted: PersistOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowHistoryEntry {
    pub started_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub latest_observed_at: DateTime<Utc>,
    pub weekly_points: f64,
    pub five_hour_estimate_percent: f64,
    pub lifecycle: String,
    pub calibration_id: String,
    pub calibration_confidence: CalibrationConfidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlEvent {
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,
    pub detail: String,
}

pub struct StateStore {
    connection: Connection,
    paths: StatePaths,
    config: TrackerConfig,
}

impl StateStore {
    pub fn open(config: TrackerConfig) -> Result<Self, StateError> {
        let paths = StatePaths::resolve(&config)?;
        Self::open_at(paths, config)
    }

    pub fn open_in(
        directory: impl Into<PathBuf>,
        config: TrackerConfig,
    ) -> Result<Self, StateError> {
        Self::open_at(StatePaths::in_directory(directory), config)
    }

    fn open_at(paths: StatePaths, config: TrackerConfig) -> Result<Self, StateError> {
        fs::create_dir_all(&paths.directory).map_err(|source| StateError::Io {
            path: paths.directory.clone(),
            source,
        })?;
        let mut connection = Connection::open(&paths.database)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        retry_busy(|| {
            connection
                .pragma_update(None, "journal_mode", "WAL")
                .map_err(StateError::from)
        })?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        retry_busy(|| migrate(&mut connection))?;
        retry_busy(|| write_config_metadata(&connection, &config))?;
        let persisted_calibration: f64 = connection.query_row(
            "SELECT calibration_weekly_points FROM config_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        let config = config.with_calibration_weekly_points(persisted_calibration)?;
        Ok(Self {
            connection,
            paths,
            config,
        })
    }

    pub fn paths(&self) -> &StatePaths {
        &self.paths
    }

    pub fn schema_version(&self) -> Result<i64, StateError> {
        Ok(self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    pub fn snapshot_count(&self) -> Result<usize, StateError> {
        let count: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub fn observation_count(&self) -> Result<usize, StateError> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM rate_limit_observations",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    pub fn diagnostic_count(&self) -> Result<usize, StateError> {
        let count: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM diagnostic_events", [], |row| {
                    row.get(0)
                })?;
        Ok(count as usize)
    }

    pub fn recent_windows(&self, limit: usize) -> Result<Vec<WindowHistoryEntry>, StateError> {
        let limit = limit.clamp(1, 100) as i64;
        let mut statement = self.connection.prepare(
            "SELECT started_at, ends_at, latest_observed_at,
                    accumulated_weekly_points, calibration_weekly_points,
                    lifecycle, calibration_id, calibration_confidence
             FROM windows ORDER BY started_at DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit], |row| {
            let weekly_points = row.get::<_, f64>(3)?;
            let calibration = row.get::<_, f64>(4)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                weekly_points,
                weekly_points / calibration * 100.0,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?;
        rows.map(|row| {
            let (start, end, observed, weekly, estimate, lifecycle, id, confidence) = row?;
            Ok(WindowHistoryEntry {
                started_at: parse_timestamp(&start)?,
                ends_at: parse_timestamp(&end)?,
                latest_observed_at: parse_timestamp(&observed)?,
                weekly_points: weekly,
                five_hour_estimate_percent: estimate,
                lifecycle,
                calibration_id: id,
                calibration_confidence: CalibrationConfidence::parse(&confidence),
            })
        })
        .collect()
    }

    pub fn recent_control_events(&self, limit: usize) -> Result<Vec<ControlEvent>, StateError> {
        let limit = limit.clamp(1, 100) as i64;
        let mut statement = self.connection.prepare(
            "SELECT occurred_at, event_type, detail
             FROM control_events ORDER BY occurred_at DESC, id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (occurred_at, event_type, detail) = row?;
            Ok(ControlEvent {
                occurred_at: parse_timestamp(&occurred_at)?,
                event_type,
                detail,
            })
        })
        .collect()
    }

    pub fn reset_current_window(&mut self, now: DateTime<Utc>) -> Result<bool, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<(String, String)> = transaction
            .query_row(
                "SELECT started_at, ends_at FROM windows WHERE lifecycle = 'current'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((started_at, old_ends_at)) = current else {
            return Ok(false);
        };
        let started = parse_timestamp(&started_at)?;
        let effective_end = now.max(started);
        transaction.execute(
            "UPDATE windows SET ends_at = ?1, lifecycle = 'archived'
             WHERE started_at = ?2 AND lifecycle = 'current'",
            params![effective_end.to_rfc3339(), started_at],
        )?;
        transaction.execute(
            "INSERT INTO control_events (occurred_at, event_type, detail)
             VALUES (?1, 'manual_reset', ?2)",
            params![
                now.to_rfc3339(),
                format!("window {started_at} shortened from {old_ends_at}")
            ],
        )?;
        transaction.commit()?;
        self.regenerate_display(now)?;
        Ok(true)
    }

    pub fn active_calibration(&self) -> f64 {
        self.selected_calibration_profile()
            .ok()
            .and_then(|profile| profile.value)
            .unwrap_or(self.config.calibration_weekly_points())
    }

    pub fn selected_calibration_profile(&self) -> Result<CalibrationProfile, StateError> {
        let identity =
            latest_calibration_identity(&self.connection)?.unwrap_or_else(|| CalibrationIdentity {
                plan_type: "unknown".to_string(),
                model_slug: "unknown".to_string(),
                service_tier: "unknown".to_string(),
                schema_fingerprint: "unavailable".to_string(),
                compatibility_generation: "unknown".to_string(),
            });
        select_calibration_profile(&self.connection, identity)
    }

    pub fn analyze_calibration(
        &mut self,
        analyzed_at: DateTime<Utc>,
    ) -> Result<CalibrationReport, StateError> {
        let observations = load_calibration_observations(&self.connection)?;
        let active_profile = self.selected_calibration_profile()?;
        let preliminary =
            build_report(observations.clone(), active_profile.clone(), analyzed_at, 0);
        let previous: Option<(i64, Option<f64>, i64)> = self
            .connection
            .query_row(
                "SELECT last_sample_count, candidate_value, confirmation_count
                 FROM calibration_analysis_state WHERE identity_key = ?1",
                [preliminary.identity.key()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let previous_count = previous.as_ref().map(|value| value.0 as usize).unwrap_or(0);
        let mut confirmation_count = previous.as_ref().map(|value| value.2 as usize).unwrap_or(0);
        if let Some(candidate) = preliminary.proposed_calibration {
            if preliminary.sample_count > previous_count {
                confirmation_count = match previous.as_ref().and_then(|value| value.1) {
                    Some(old) if ((candidate - old) / old).abs() <= 0.02 => {
                        confirmation_count.saturating_add(1)
                    }
                    _ => 1,
                };
            }
        } else {
            confirmation_count = 0;
        }
        let report = build_report(
            observations,
            active_profile,
            analyzed_at,
            confirmation_count,
        );
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for sample in &report.samples {
            transaction.execute(
                "INSERT OR IGNORE INTO calibration_samples (
                     sample_key, window_started_at, window_ended_at,
                     five_hour_reset_at, weekly_reset_at, five_hour_delta_percent,
                     weekly_delta_points, implied_weekly_points,
                     predicted_five_hour_percent, prediction_error_percent,
                     model_slug, service_tier, plan_type, created_at,
                     identity_key, schema_fingerprint, compatibility_generation,
                     quality, eligible_for_estimator, diagnostic_reason
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                           ?15, ?16, ?17, ?18, ?19, ?20)",
                params![
                    sample.sample_key,
                    sample.window_started_at.to_rfc3339(),
                    sample.window_ended_at.to_rfc3339(),
                    sample.five_hour_reset_at.to_rfc3339(),
                    sample.weekly_reset_at.to_rfc3339(),
                    sample.five_hour_delta_percent,
                    sample.weekly_delta_points,
                    sample.implied_weekly_points,
                    sample.predicted_five_hour_percent,
                    sample.prediction_error_percent,
                    sample.identity.model_slug,
                    sample.identity.service_tier,
                    sample.identity.plan_type,
                    analyzed_at.to_rfc3339(),
                    sample.identity.key(),
                    sample.identity.schema_fingerprint,
                    sample.identity.compatibility_generation,
                    format!("{:?}", sample.quality).to_lowercase(),
                    sample.eligible_for_estimator,
                    sample.diagnostic_reason,
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO calibration_analysis_state (
                 identity_key, last_sample_count, candidate_value,
                 confirmation_count, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(identity_key) DO UPDATE SET
                 last_sample_count = excluded.last_sample_count,
                 candidate_value = excluded.candidate_value,
                 confirmation_count = excluded.confirmation_count,
                 updated_at = excluded.updated_at",
            params![
                report.identity.key(),
                report.sample_count as i64,
                report.proposed_calibration,
                report.drift_confirmation_count as i64,
                analyzed_at.to_rfc3339(),
            ],
        )?;
        transaction.commit()?;
        Ok(report)
    }

    pub fn calibration_sample_count(&self) -> Result<usize, StateError> {
        let count: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM calibration_samples", [], |row| {
                    row.get(0)
                })?;
        Ok(count as usize)
    }

    pub fn maybe_generate_calibration_report(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<Option<(String, CalibrationReport)>, StateError> {
        let report = self.analyze_calibration(now)?;
        let previous: Option<(String, i64)> = self
            .connection
            .query_row(
                "SELECT generated_at, sample_count FROM calibration_reports
                 WHERE identity_key = ?1 ORDER BY generated_at DESC, rowid DESC LIMIT 1",
                [report.identity.key()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let reason = match previous {
            None => "initial",
            Some((_, count)) if report.sample_count >= count as usize + 5 => {
                "five_new_qualifying_windows"
            }
            Some((generated_at, _))
                if now - parse_timestamp(&generated_at)? >= chrono::TimeDelta::days(7) =>
            {
                "weekly"
            }
            Some(_) => return Ok(None),
        };
        let report_json = serde_json::to_string_pretty(&report)?;
        let report_key = format!(
            "{}|{}|{}|{}",
            report.identity.key(),
            reason,
            report.sample_count,
            now.date_naive()
        );
        self.connection.execute(
            "INSERT OR IGNORE INTO calibration_reports (
                 report_key, identity_key, generated_at, reason,
                 sample_count, report_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                report_key,
                report.identity.key(),
                now.to_rfc3339(),
                reason,
                report.sample_count as i64,
                report_json,
            ],
        )?;
        write_json_atomic(
            &self.paths.directory,
            &self.paths.calibration_report,
            &report,
        )?;
        Ok(Some((reason.to_string(), report)))
    }

    pub fn apply_calibration(
        &mut self,
        weekly_points: f64,
        approved_at: DateTime<Utc>,
    ) -> Result<String, StateError> {
        self.config.with_calibration_weekly_points(weekly_points)?;
        let report = self.analyze_calibration(approved_at)?;
        let profile = self.selected_calibration_profile()?;
        if profile.confidence == CalibrationConfidence::Unsupported {
            return Err(StateError::UnsupportedCalibrationIdentity);
        }
        let calibration_id = stable_calibration_id(
            &format!("user-approved:{}", approved_at.to_rfc3339()),
            &profile.identity,
            weekly_points,
        );
        let confidence = if report.confidence == CalibrationConfidence::PersonalValidated {
            CalibrationConfidence::PersonalValidated
        } else {
            CalibrationConfidence::PersonalCandidate
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO calibration_profiles (
                 calibration_id, identity_key, plan_type, model_slug, service_tier,
                 schema_fingerprint, compatibility_generation, value, confidence,
                 source, evidence_period_start, evidence_period_end, approved_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                calibration_id,
                profile.identity.key(),
                profile.identity.plan_type,
                profile.identity.model_slug,
                profile.identity.service_tier,
                profile.identity.schema_fingerprint,
                profile.identity.compatibility_generation,
                weekly_points,
                confidence.as_str(),
                "explicit user approval",
                report.data_period_start.map(|value| value.to_rfc3339()),
                report.data_period_end.map(|value| value.to_rfc3339()),
                approved_at.to_rfc3339()
            ],
        )?;
        transaction.execute(
            "INSERT INTO calibration_applications (
                 applied_at, calibration_weekly_points, sample_count,
                 calibration_id, identity_key
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                approved_at.to_rfc3339(),
                weekly_points,
                report.sample_count as i64,
                calibration_id,
                profile.identity.key()
            ],
        )?;
        transaction.commit()?;
        Ok(calibration_id)
    }

    pub fn backup_database(&self, destination: &Path) -> Result<(), StateError> {
        if destination.exists() {
            return Err(StateError::Io {
                path: destination.to_path_buf(),
                source: io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "backup destination already exists",
                ),
            });
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source| StateError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        self.connection
            .pragma_query_value(None, "wal_checkpoint", |row| row.get::<_, i64>(0))?;
        self.connection.execute(
            "VACUUM main INTO ?1",
            [destination.to_string_lossy().as_ref()],
        )?;
        Ok(())
    }

    pub fn current_compatibility_identity(
        &self,
        codex_version: Option<&str>,
        model_slug: Option<&str>,
        service_tier: Option<&str>,
    ) -> Result<(CompatibilityIdentity, bool), StateError> {
        type LatestCompatibilityObservation = (
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            String,
        );
        let latest: Option<LatestCompatibilityObservation> = self
            .connection
            .query_row(
                "SELECT plan_type, model_slug, service_tier, schema_fingerprint, observed_at
                 FROM rate_limit_observations
                 ORDER BY observed_at DESC, source_file DESC, byte_offset DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let latest_diagnostic: Option<(String, Option<String>)> = self
            .connection
            .query_row(
                "SELECT code, observed_at FROM diagnostic_events
                 WHERE code LIKE 'unsupported_%' OR code IN ('missing_rate_limits', 'missing_weekly_window')
                 ORDER BY COALESCE(observed_at, '') DESC, rowid DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let diagnostic_is_newer = match (&latest, &latest_diagnostic) {
            (Some((_, _, _, _, observed)), Some((_, Some(diagnostic_at)))) => {
                diagnostic_at >= observed
            }
            (None, Some(_)) => true,
            _ => false,
        };
        let (observed_plan, observed_model, observed_tier, fingerprint) = latest
            .map(|(plan, model, tier, fingerprint, _)| (plan, model, tier, fingerprint))
            .unwrap_or((None, None, None, "unavailable".to_string()));
        let schema_supported = fingerprint != "unavailable" && !diagnostic_is_newer;
        let fingerprint = if diagnostic_is_newer {
            format!(
                "unsupported:{}",
                latest_diagnostic
                    .as_ref()
                    .map(|value| value.0.as_str())
                    .unwrap_or("unknown")
            )
        } else {
            fingerprint
        };
        Ok((
            CompatibilityIdentity {
                codex_version: codex_version.unwrap_or("unknown").to_string(),
                plan_type: observed_plan.unwrap_or_else(|| "unknown".to_string()),
                model_slug: model_slug
                    .map(str::to_string)
                    .or(observed_model)
                    .unwrap_or_else(|| "unknown".to_string()),
                service_tier: service_tier
                    .map(str::to_string)
                    .or(observed_tier)
                    .unwrap_or_else(|| "unknown".to_string()),
                schema_fingerprint: fingerprint,
            },
            schema_supported,
        ))
    }

    pub fn check_compatibility(
        &mut self,
        identity: CompatibilityIdentity,
        schema_supported: bool,
        checked_at: DateTime<Utc>,
    ) -> Result<CompatibilityCheck, StateError> {
        let prior_profile = self.selected_calibration_profile().ok();
        if let Some(existing) = self
            .connection
            .query_row(
                "SELECT result, model_confidence, hook_check, transcript_check,
                        rate_limit_check, projection_check, tracker_version,
                        plugin_version, checked_at
                 FROM compatibility_results WHERE identity_key = ?1",
                [identity.key()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()?
        {
            return Ok(CompatibilityCheck {
                identity,
                result: CompatibilityResult::parse(&existing.0),
                first_seen: false,
                model_confidence: existing.1,
                hook_check: existing.2,
                transcript_check: existing.3,
                rate_limit_check: existing.4,
                projection_check: existing.5,
                tracker_version: existing.6,
                plugin_version: existing.7,
                checked_at: parse_timestamp(&existing.8)?,
            });
        }

        let prior_identity_count: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM compatibility_results", [], |row| {
                    row.get(0)
                })?;
        let prior_model_count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM compatibility_results WHERE model_slug = ?1",
            [identity.model_slug.as_str()],
            |row| row.get(0),
        )?;
        let new_model = prior_identity_count > 0 && prior_model_count == 0;
        let result = if !schema_supported {
            CompatibilityResult::Degraded
        } else if new_model {
            CompatibilityResult::Review
        } else {
            CompatibilityResult::Compatible
        };
        let model_confidence = if new_model {
            "inherited / not validated for this model"
        } else {
            "historical"
        };
        let rate_limit_check = if schema_supported {
            "compatible"
        } else {
            "unsupported; local estimate unavailable"
        };
        let check = CompatibilityCheck {
            identity: identity.clone(),
            result,
            first_seen: true,
            model_confidence: model_confidence.to_string(),
            hook_check: "compatible".to_string(),
            transcript_check: "incremental parser available".to_string(),
            rate_limit_check: rate_limit_check.to_string(),
            projection_check: "display projection v1 supported".to_string(),
            tracker_version: env!("CARGO_PKG_VERSION").to_string(),
            plugin_version: "1".to_string(),
            checked_at,
        };
        self.connection.execute(
            "INSERT INTO compatibility_results (
                 identity_key, codex_version, plan_type, model_slug, service_tier,
                 schema_fingerprint, result, model_confidence, hook_check,
                 transcript_check, rate_limit_check, projection_check,
                 tracker_version, plugin_version, checked_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                identity.key(),
                identity.codex_version,
                identity.plan_type,
                identity.model_slug,
                identity.service_tier,
                identity.schema_fingerprint,
                result.as_str(),
                check.model_confidence,
                check.hook_check,
                check.transcript_check,
                check.rate_limit_check,
                check.projection_check,
                check.tracker_version,
                check.plugin_version,
                checked_at.to_rfc3339(),
            ],
        )?;
        let generation = identity.key();
        self.connection.execute(
            "UPDATE config_metadata SET current_compatibility_generation = ?1,
                 updated_at = ?2 WHERE singleton = 1",
            params![generation, checked_at.to_rfc3339()],
        )?;
        if result == CompatibilityResult::Review {
            if let Some(previous) = prior_profile {
                if let Some(value) = previous.value {
                    let inherited_identity = CalibrationIdentity {
                        plan_type: identity.plan_type.clone(),
                        model_slug: identity.model_slug.clone(),
                        service_tier: identity.service_tier.clone(),
                        schema_fingerprint: identity.schema_fingerprint.clone(),
                        compatibility_generation: generation,
                    };
                    let calibration_id = stable_calibration_id(
                        &format!("inherited-from:{}", previous.calibration_id),
                        &inherited_identity,
                        value,
                    );
                    self.connection.execute(
                        "INSERT OR IGNORE INTO calibration_profiles (
                     calibration_id, identity_key, plan_type, model_slug, service_tier,
                     schema_fingerprint, compatibility_generation, value, confidence,
                     source, evidence_period_start, evidence_period_end, approved_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                        params![
                            calibration_id,
                            inherited_identity.key(),
                            inherited_identity.plan_type,
                            inherited_identity.model_slug,
                            inherited_identity.service_tier,
                            inherited_identity.schema_fingerprint,
                            inherited_identity.compatibility_generation,
                            value,
                            CalibrationConfidence::InheritedUnvalidated.as_str(),
                            format!(
                                "inherited from {} after compatibility change",
                                previous.calibration_id
                            ),
                            previous
                                .evidence_period_start
                                .map(|value| value.to_rfc3339()),
                            previous.evidence_period_end.map(|value| value.to_rfc3339()),
                            checked_at.to_rfc3339(),
                        ],
                    )?;
                }
            }
        }
        Ok(check)
    }

    pub fn transcript_cursor(&self, path: &Path) -> Result<TranscriptCursor, StateError> {
        Ok(self.transcript_cursor_state(path)?.0)
    }

    fn transcript_cursor_state(
        &self,
        path: &Path,
    ) -> Result<(TranscriptCursor, Vec<u8>, u64), StateError> {
        let canonical = fs::canonicalize(path).map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let row: Option<(i64, Vec<u8>, i64)> = self
            .connection
            .query_row(
                "SELECT next_offset, continuity_marker, generation
                 FROM transcript_cursors WHERE source_file = ?1",
                [canonical.to_string_lossy().as_ref()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let (offset, marker, generation) = row.unwrap_or((0, Vec::new(), 0));
        Ok((
            TranscriptCursor {
                next_offset: u64::try_from(offset).unwrap_or(0),
            },
            marker,
            u64::try_from(generation).unwrap_or(0),
        ))
    }

    /// Reads only the uncommitted suffix of a transcript and atomically stores
    /// observations, diagnostics, weekly snapshots, and the matching cursor.
    pub fn ingest_transcript(
        &mut self,
        path: &Path,
        options: &IngestOptions,
    ) -> Result<PersistTranscriptOutcome, StateError> {
        let (mut cursor, saved_marker, mut generation) = self.transcript_cursor_state(path)?;
        if cursor.next_offset > 0 && !saved_marker.is_empty() {
            let current_marker = transcript_continuity_marker(path, cursor.next_offset)?;
            if current_marker.as_deref() != Some(saved_marker.as_slice()) {
                cursor.next_offset = 0;
                generation = generation.saturating_add(1);
            }
        }
        let mut batch = ingest_transcript(path, cursor, options)?;
        apply_transcript_generation(&mut batch, generation);
        let marker = transcript_continuity_marker(path, batch.next_offset)?.unwrap_or_default();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted_snapshots = insert_snapshots(&transaction, &batch.snapshots)?;
        let inserted_observations = insert_observations(&transaction, &batch.observations)?;
        let inserted_diagnostics = insert_diagnostics(&transaction, &batch.diagnostics)?;
        transaction.execute(
            "INSERT INTO transcript_cursors (
                 source_file, next_offset, updated_at, continuity_marker, generation
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(source_file) DO UPDATE SET
                 next_offset = excluded.next_offset,
                 updated_at = excluded.updated_at,
                 continuity_marker = excluded.continuity_marker,
                 generation = excluded.generation",
            params![
                batch.source_file.to_string_lossy(),
                i64::try_from(batch.next_offset).unwrap_or(i64::MAX),
                options.now.to_rfc3339(),
                marker,
                i64::try_from(generation).unwrap_or(i64::MAX),
            ],
        )?;
        let replay = rebuild_windows(&transaction, &self.config, options.now)?;
        transaction.commit()?;
        let display = self.regenerate_display(options.now)?;
        Ok(PersistTranscriptOutcome {
            batch,
            inserted_observations,
            inserted_diagnostics,
            persisted: PersistOutcome {
                inserted_snapshots,
                newly_emitted_warnings: replay.new_warnings,
                display,
            },
        })
    }

    pub fn ingest(
        &mut self,
        snapshots: impl IntoIterator<Item = WeeklySnapshot>,
        now: DateTime<Utc>,
    ) -> Result<PersistOutcome, StateError> {
        let mut snapshots: Vec<_> = snapshots.into_iter().collect();
        snapshots.sort_by(|left, right| {
            (&left.observed_at, &left.id).cmp(&(&right.observed_at, &right.id))
        });

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted_snapshots = insert_snapshots(&transaction, &snapshots)?;
        let replay = rebuild_windows(&transaction, &self.config, now)?;
        transaction.commit()?;

        // A second short write lock serializes projection writers. It is taken
        // after the state commit so the file can only describe committed data.
        let display = self.regenerate_display(now)?;
        Ok(PersistOutcome {
            inserted_snapshots,
            newly_emitted_warnings: replay.new_warnings,
            display,
        })
    }

    pub fn load_or_recover_display(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<DisplayCacheV1, StateError> {
        match fs::read(&self.paths.display) {
            Ok(bytes) => match serde_json::from_slice::<DisplayCacheV1>(&bytes) {
                Ok(display) if display.schema_version == DISPLAY_SCHEMA_VERSION => Ok(display),
                _ => self.regenerate_display(now),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => self.regenerate_display(now),
            Err(source) => Err(StateError::Io {
                path: self.paths.display.clone(),
                source,
            }),
        }
    }

    fn regenerate_display(&mut self, now: DateTime<Utc>) -> Result<DisplayCacheV1, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let display = read_display(&transaction, &self.config, now)?;
        write_display_atomic(&self.paths, &display)?;
        transaction.commit()?;
        Ok(display)
    }
}

fn transcript_continuity_marker(path: &Path, offset: u64) -> Result<Option<Vec<u8>>, StateError> {
    let mut file = fs::File::open(path).map_err(|source| StateError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let length = file
        .metadata()
        .map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    if offset > length {
        return Ok(None);
    }

    const SAMPLE: u64 = 128;
    let first_len = offset.min(SAMPLE) as usize;
    let tail_start = offset.saturating_sub(SAMPLE);
    let tail_len = (offset - tail_start) as usize;
    let mut marker = Vec::with_capacity(16 + first_len + tail_len);
    marker.extend_from_slice(&offset.to_le_bytes());
    let mut buffer = vec![0; first_len];
    file.read_exact(&mut buffer)
        .map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    marker.extend_from_slice(&buffer);
    file.seek(SeekFrom::Start(tail_start))
        .and_then(|_| {
            buffer.resize(tail_len, 0);
            file.read_exact(&mut buffer)
        })
        .map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    marker.extend_from_slice(&buffer);
    Ok(Some(marker))
}

fn apply_transcript_generation(batch: &mut IngestBatch, generation: u64) {
    let logical_source = PathBuf::from(format!(
        "{}#codex-usage-watch-generation={generation}",
        batch.source_file.to_string_lossy()
    ));
    for snapshot in &mut batch.snapshots {
        snapshot.id.source_file = logical_source.clone();
    }
    for observation in &mut batch.observations {
        observation.id.source_file = logical_source.clone();
    }
    for diagnostic in &mut batch.diagnostics {
        diagnostic.id.source_file = logical_source.clone();
    }
}

fn retry_busy<T>(mut operation: impl FnMut() -> Result<T, StateError>) -> Result<T, StateError> {
    let deadline = Instant::now() + BUSY_TIMEOUT;
    loop {
        match operation() {
            Err(error) if is_busy(&error) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            result => return result,
        }
    }
}

fn is_busy(error: &StateError) -> bool {
    matches!(
        error,
        StateError::Sqlite(rusqlite::Error::SqliteFailure(details, _))
            if matches!(
                details.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
    )
}

fn migrate(connection: &mut Connection) -> Result<(), StateError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(StateError::UnsupportedSchemaVersion {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    if version < 1 {
        transaction.execute_batch(
            "CREATE TABLE schema_migrations (
                 version INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL
             );
             CREATE TABLE config_metadata (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 calibration_weekly_points REAL NOT NULL CHECK (calibration_weekly_points > 0),
                 local_window_seconds INTEGER NOT NULL CHECK (local_window_seconds > 0),
                 stale_after_seconds INTEGER NOT NULL CHECK (stale_after_seconds > 0),
                 warning_thresholds_json TEXT NOT NULL,
                 super_usage_step INTEGER NOT NULL CHECK (super_usage_step > 0),
                 calibration_kind TEXT NOT NULL,
                 updated_at TEXT NOT NULL
             );
             CREATE TABLE snapshots (
                 id INTEGER PRIMARY KEY,
                 source_file TEXT NOT NULL,
                 byte_offset INTEGER NOT NULL CHECK (byte_offset >= 0),
                 observed_at TEXT NOT NULL,
                 used_percent REAL NOT NULL CHECK (used_percent >= 0 AND used_percent <= 100),
                 resets_at TEXT,
                 window_minutes INTEGER NOT NULL CHECK (window_minutes = 10080),
                 plan_type TEXT,
                 affects_meter INTEGER NOT NULL DEFAULT 0 CHECK (affects_meter IN (0, 1)),
                 UNIQUE (source_file, byte_offset)
             );
             CREATE INDEX snapshots_order ON snapshots(observed_at, source_file, byte_offset);
             CREATE TABLE windows (
                 started_at TEXT PRIMARY KEY,
                 ends_at TEXT NOT NULL,
                 latest_observed_at TEXT NOT NULL,
                 latest_used_percent REAL NOT NULL,
                 calibration_weekly_points REAL NOT NULL CHECK (calibration_weekly_points > 0),
                 accumulated_weekly_points REAL NOT NULL CHECK (accumulated_weekly_points >= 0),
                 last_emitted_milestone INTEGER,
                 lifecycle TEXT NOT NULL CHECK (lifecycle IN ('current', 'archived'))
             );
             CREATE UNIQUE INDEX one_current_window ON windows(lifecycle) WHERE lifecycle = 'current';
             CREATE TABLE emitted_warnings (
                 window_started_at TEXT NOT NULL,
                 milestone INTEGER NOT NULL,
                 emitted_at TEXT NOT NULL,
                 PRIMARY KEY (window_started_at, milestone)
             );"
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![1, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", 1)?;
    }
    if version < 2 {
        transaction.execute_batch(
            "CREATE TABLE rate_limit_observations (
                 source_file TEXT NOT NULL,
                 byte_offset INTEGER NOT NULL CHECK (byte_offset >= 0),
                 observed_at TEXT NOT NULL,
                 model_slug TEXT,
                 service_tier TEXT,
                 plan_type TEXT,
                 schema_fingerprint TEXT NOT NULL,
                 PRIMARY KEY (source_file, byte_offset)
             );
             CREATE INDEX observations_order
                 ON rate_limit_observations(observed_at, source_file, byte_offset);
             CREATE TABLE observed_rate_limit_windows (
                 source_file TEXT NOT NULL,
                 byte_offset INTEGER NOT NULL CHECK (byte_offset >= 0),
                 window_kind TEXT NOT NULL CHECK (window_kind IN ('five_hour', 'weekly')),
                 used_percent REAL NOT NULL CHECK (used_percent >= 0 AND used_percent <= 100),
                 window_minutes INTEGER NOT NULL CHECK (window_minutes IN (300, 10080)),
                 resets_at TEXT,
                 PRIMARY KEY (source_file, byte_offset, window_kind),
                 FOREIGN KEY (source_file, byte_offset)
                     REFERENCES rate_limit_observations(source_file, byte_offset)
                     ON DELETE CASCADE
             );
             CREATE TABLE diagnostic_events (
                 source_file TEXT NOT NULL,
                 byte_offset INTEGER NOT NULL CHECK (byte_offset >= 0),
                 observed_at TEXT,
                 code TEXT NOT NULL,
                 field TEXT NOT NULL DEFAULT '',
                 message TEXT NOT NULL,
                 PRIMARY KEY (source_file, byte_offset, code, field)
             );
             CREATE TABLE transcript_cursors (
                 source_file TEXT PRIMARY KEY,
                 next_offset INTEGER NOT NULL CHECK (next_offset >= 0),
                 updated_at TEXT NOT NULL
             );",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![2, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    if version < 3 {
        transaction.execute_batch(
            "ALTER TABLE transcript_cursors
                 ADD COLUMN continuity_marker BLOB NOT NULL DEFAULT X'';
             ALTER TABLE transcript_cursors
                 ADD COLUMN generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0);",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![3, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    if version < 4 {
        transaction.execute_batch(
            "CREATE TABLE calibration_samples (
                 sample_key TEXT PRIMARY KEY,
                 window_started_at TEXT NOT NULL,
                 window_ended_at TEXT NOT NULL,
                 five_hour_reset_at TEXT NOT NULL,
                 weekly_reset_at TEXT NOT NULL,
                 five_hour_delta_percent REAL NOT NULL CHECK (five_hour_delta_percent > 0),
                 weekly_delta_points REAL NOT NULL CHECK (weekly_delta_points >= 0),
                 implied_weekly_points REAL NOT NULL CHECK (implied_weekly_points > 0),
                 model_slug TEXT,
                 service_tier TEXT,
                 plan_type TEXT,
                 created_at TEXT NOT NULL
             );
             CREATE TABLE calibration_applications (
                 id INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL,
                 calibration_weekly_points REAL NOT NULL CHECK (calibration_weekly_points > 0),
                 sample_count INTEGER NOT NULL CHECK (sample_count >= 0)
             );",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![4, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    if version < 5 {
        transaction.execute_batch(
            "CREATE TABLE compatibility_results (
                 identity_key TEXT PRIMARY KEY,
                 codex_version TEXT NOT NULL,
                 model_slug TEXT NOT NULL,
                 service_tier TEXT NOT NULL,
                 schema_fingerprint TEXT NOT NULL,
                 result TEXT NOT NULL CHECK (result IN ('compatible', 'review', 'degraded')),
                 model_confidence TEXT NOT NULL,
                 hook_check TEXT NOT NULL,
                 transcript_check TEXT NOT NULL,
                 rate_limit_check TEXT NOT NULL,
                 projection_check TEXT NOT NULL,
                 tracker_version TEXT NOT NULL,
                 plugin_version TEXT NOT NULL,
                 checked_at TEXT NOT NULL
             );",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![5, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    if version < 6 {
        transaction.execute_batch(
            "ALTER TABLE calibration_samples
                 ADD COLUMN predicted_five_hour_percent REAL NOT NULL DEFAULT 0;
             ALTER TABLE calibration_samples
                 ADD COLUMN prediction_error_percent REAL NOT NULL DEFAULT 0;",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![6, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    if version < 7 {
        transaction.execute_batch(
            "ALTER TABLE config_metadata
                 ADD COLUMN current_compatibility_generation TEXT NOT NULL DEFAULT 'unknown';
             ALTER TABLE rate_limit_observations
                 ADD COLUMN compatibility_generation TEXT NOT NULL DEFAULT 'unknown';
             ALTER TABLE windows
                 ADD COLUMN calibration_id TEXT NOT NULL DEFAULT 'legacy-historical-15.8';
             ALTER TABLE windows
                 ADD COLUMN calibration_confidence TEXT NOT NULL DEFAULT 'inherited_unvalidated';
             ALTER TABLE calibration_samples
                 ADD COLUMN identity_key TEXT NOT NULL DEFAULT 'legacy';
             ALTER TABLE calibration_samples
                 ADD COLUMN schema_fingerprint TEXT NOT NULL DEFAULT 'legacy';
             ALTER TABLE calibration_samples
                 ADD COLUMN compatibility_generation TEXT NOT NULL DEFAULT 'unknown';
             ALTER TABLE calibration_samples
                 ADD COLUMN quality TEXT NOT NULL DEFAULT 'ignored';
             ALTER TABLE calibration_samples
                 ADD COLUMN eligible_for_estimator INTEGER NOT NULL DEFAULT 0 CHECK (eligible_for_estimator IN (0, 1));
             ALTER TABLE calibration_samples
                 ADD COLUMN diagnostic_reason TEXT NOT NULL DEFAULT 'legacy sample; reanalysis required';
             ALTER TABLE calibration_applications
                 ADD COLUMN calibration_id TEXT NOT NULL DEFAULT 'legacy';
             ALTER TABLE calibration_applications
                 ADD COLUMN identity_key TEXT NOT NULL DEFAULT 'legacy';
             ALTER TABLE compatibility_results
                 ADD COLUMN plan_type TEXT NOT NULL DEFAULT 'unknown';
             CREATE TABLE calibration_profiles (
                 calibration_id TEXT PRIMARY KEY,
                 identity_key TEXT NOT NULL,
                 plan_type TEXT NOT NULL,
                 model_slug TEXT NOT NULL,
                 service_tier TEXT NOT NULL,
                 schema_fingerprint TEXT NOT NULL,
                 compatibility_generation TEXT NOT NULL,
                 value REAL NOT NULL CHECK (value > 0),
                 confidence TEXT NOT NULL CHECK (confidence IN (
                     'baseline', 'personal_preliminary', 'personal_candidate',
                     'personal_validated', 'inherited_unvalidated', 'unsupported'
                 )),
                 source TEXT NOT NULL,
                 evidence_period_start TEXT,
                 evidence_period_end TEXT,
                 approved_at TEXT NOT NULL
             );
             CREATE INDEX calibration_profiles_identity
                 ON calibration_profiles(identity_key, approved_at DESC);
             CREATE TABLE calibration_analysis_state (
                 identity_key TEXT PRIMARY KEY,
                 last_sample_count INTEGER NOT NULL CHECK (last_sample_count >= 0),
                 candidate_value REAL,
                 confirmation_count INTEGER NOT NULL CHECK (confirmation_count >= 0),
                 updated_at TEXT NOT NULL
             );",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![7, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    if version < 8 {
        transaction.execute_batch(
            "CREATE TABLE calibration_reports (
                 report_key TEXT PRIMARY KEY,
                 identity_key TEXT NOT NULL,
                 generated_at TEXT NOT NULL,
                 reason TEXT NOT NULL CHECK (reason IN (
                     'initial', 'weekly', 'five_new_qualifying_windows'
                 )),
                 sample_count INTEGER NOT NULL CHECK (sample_count >= 0),
                 report_json TEXT NOT NULL
             );
             CREATE INDEX calibration_reports_identity
                 ON calibration_reports(identity_key, generated_at DESC);",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![8, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    if version < 9 {
        transaction.execute_batch(
            "ALTER TABLE rate_limit_observations
                 ADD COLUMN codex_version TEXT;",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![9, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    if version < 10 {
        transaction.execute_batch(
            "CREATE TABLE control_events (
                 id INTEGER PRIMARY KEY,
                 occurred_at TEXT NOT NULL,
                 event_type TEXT NOT NULL CHECK (event_type IN ('manual_reset')),
                 detail TEXT NOT NULL
             );
             CREATE INDEX control_events_time
                 ON control_events(occurred_at DESC);",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![10, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    transaction.commit()?;
    Ok(())
}

fn insert_observations(
    transaction: &Transaction<'_>,
    observations: &[UsageObservation],
) -> Result<usize, StateError> {
    let mut inserted = 0;
    let compatibility_generation: String = transaction.query_row(
        "SELECT current_compatibility_generation FROM config_metadata WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    for observation in observations {
        inserted += transaction.execute(
            "INSERT OR IGNORE INTO rate_limit_observations (
                 source_file, byte_offset, observed_at, model_slug, service_tier,
                 plan_type, schema_fingerprint, compatibility_generation,
                 codex_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                observation.id.source_file.to_string_lossy(),
                i64::try_from(observation.id.byte_offset).unwrap_or(i64::MAX),
                observation.observed_at.to_rfc3339(),
                observation.model_slug,
                observation.service_tier,
                observation.plan_type,
                observation.schema_fingerprint,
                observation
                    .codex_version
                    .as_ref()
                    .map(|version| format!("codex-version:{version}"))
                    .unwrap_or_else(|| compatibility_generation.clone()),
                observation.codex_version,
            ],
        )?;
        for (kind, window) in [
            ("five_hour", observation.five_hour.as_ref()),
            ("weekly", observation.weekly.as_ref()),
        ] {
            if let Some(window) = window {
                transaction.execute(
                    "INSERT OR IGNORE INTO observed_rate_limit_windows (
                         source_file, byte_offset, window_kind, used_percent,
                         window_minutes, resets_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        observation.id.source_file.to_string_lossy(),
                        i64::try_from(observation.id.byte_offset).unwrap_or(i64::MAX),
                        kind,
                        window.used_percent,
                        window.window_minutes,
                        window.resets_at.map(|value| value.to_rfc3339()),
                    ],
                )?;
            }
        }
    }
    Ok(inserted)
}

fn insert_diagnostics(
    transaction: &Transaction<'_>,
    diagnostics: &[IngestDiagnostic],
) -> Result<usize, StateError> {
    let mut inserted = 0;
    for diagnostic in diagnostics {
        inserted += transaction.execute(
            "INSERT OR IGNORE INTO diagnostic_events (
                 source_file, byte_offset, observed_at, code, field, message
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                diagnostic.id.source_file.to_string_lossy(),
                i64::try_from(diagnostic.id.byte_offset).unwrap_or(i64::MAX),
                diagnostic.observed_at.map(|value| value.to_rfc3339()),
                diagnostic.code,
                diagnostic.field.as_deref().unwrap_or(""),
                diagnostic.message,
            ],
        )?;
    }
    Ok(inserted)
}

fn write_config_metadata(
    connection: &Connection,
    config: &TrackerConfig,
) -> Result<(), StateError> {
    connection.execute(
        "INSERT INTO config_metadata (
             singleton, calibration_weekly_points, local_window_seconds,
             stale_after_seconds, warning_thresholds_json, super_usage_step,
             calibration_kind, updated_at
         ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(singleton) DO UPDATE SET
             local_window_seconds = excluded.local_window_seconds,
             stale_after_seconds = excluded.stale_after_seconds,
             warning_thresholds_json = excluded.warning_thresholds_json,
             super_usage_step = excluded.super_usage_step,
             updated_at = excluded.updated_at",
        params![
            config.calibration_weekly_points(),
            config.local_window_duration().num_seconds(),
            config.stale_after().num_seconds(),
            serde_json::to_string(config.warning_thresholds())?,
            config.super_usage_step(),
            CalibrationKind::Historical.as_str(),
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn latest_calibration_identity(
    connection: &Connection,
) -> Result<Option<CalibrationIdentity>, StateError> {
    connection
        .query_row(
            "SELECT COALESCE(plan_type, 'unknown'), COALESCE(model_slug, 'unknown'),
                    COALESCE(service_tier, 'unknown'), schema_fingerprint,
                    compatibility_generation
             FROM rate_limit_observations
             ORDER BY observed_at DESC, source_file DESC, byte_offset DESC LIMIT 1",
            [],
            |row| {
                Ok(CalibrationIdentity {
                    plan_type: row.get(0)?,
                    model_slug: row.get(1)?,
                    service_tier: row.get(2)?,
                    schema_fingerprint: row.get(3)?,
                    compatibility_generation: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(StateError::from)
}

fn select_calibration_profile(
    connection: &Connection,
    identity: CalibrationIdentity,
) -> Result<CalibrationProfile, StateError> {
    type ProfileRow = (
        String,
        f64,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
    );
    let row: Option<ProfileRow> = connection
        .query_row(
            "SELECT calibration_id, value, confidence, source,
                    evidence_period_start, evidence_period_end, approved_at
             FROM calibration_profiles WHERE identity_key = ?1
             ORDER BY approved_at DESC, rowid DESC LIMIT 1",
            [identity.key()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()?;
    if let Some((id, value, confidence, source, start, end, approved)) = row {
        return Ok(CalibrationProfile {
            calibration_id: id,
            identity,
            value: Some(value),
            confidence: CalibrationConfidence::parse(&confidence),
            source,
            evidence_period_start: start.as_deref().map(parse_timestamp).transpose()?,
            evidence_period_end: end.as_deref().map(parse_timestamp).transpose()?,
            approved_at: Some(parse_timestamp(&approved)?),
        });
    }
    if identity.supports_plus_baseline() {
        Ok(CalibrationProfile::plus_baseline(identity))
    } else {
        Ok(CalibrationProfile::unsupported(identity))
    }
}

fn load_calibration_observations(
    connection: &Connection,
) -> Result<Vec<CalibrationObservation>, StateError> {
    type Row = (
        String,
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        String,
        Option<f64>,
        Option<String>,
        Option<f64>,
        Option<String>,
    );
    let mut statement = connection.prepare(
        "SELECT o.source_file, o.byte_offset, o.observed_at, o.model_slug,
                o.service_tier, o.plan_type, o.schema_fingerprint,
                o.compatibility_generation,
                MAX(CASE WHEN w.window_kind = 'five_hour' THEN w.used_percent END),
                MAX(CASE WHEN w.window_kind = 'five_hour' THEN w.resets_at END),
                MAX(CASE WHEN w.window_kind = 'weekly' THEN w.used_percent END),
                MAX(CASE WHEN w.window_kind = 'weekly' THEN w.resets_at END)
         FROM rate_limit_observations o
         LEFT JOIN observed_rate_limit_windows w
           ON w.source_file = o.source_file AND w.byte_offset = o.byte_offset
         GROUP BY o.source_file, o.byte_offset
         ORDER BY o.observed_at, o.source_file, o.byte_offset",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
            row.get(10)?,
            row.get(11)?,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (
            source,
            offset,
            observed_at,
            model_slug,
            service_tier,
            plan_type,
            schema_fingerprint,
            compatibility_generation,
            five_used,
            five_reset,
            weekly_used,
            weekly_reset,
        ): Row = row?;
        let five_reset = five_reset.as_deref().map(parse_timestamp).transpose()?;
        let weekly_reset = weekly_reset.as_deref().map(parse_timestamp).transpose()?;
        result.push(CalibrationObservation {
            source_key: format!("{source}:{offset}"),
            observed_at: parse_timestamp(&observed_at)?,
            five_hour: five_used.map(|used_percent| ObservedRateLimitWindow {
                used_percent,
                window_minutes: crate::model::FIVE_HOUR_WINDOW_MINUTES,
                resets_at: five_reset,
            }),
            weekly: weekly_used.map(|used_percent| ObservedRateLimitWindow {
                used_percent,
                window_minutes: crate::model::WEEKLY_WINDOW_MINUTES,
                resets_at: weekly_reset,
            }),
            identity: CalibrationIdentity {
                plan_type: plan_type.unwrap_or_else(|| "unknown".to_string()),
                model_slug: model_slug.unwrap_or_else(|| "unknown".to_string()),
                service_tier: service_tier.unwrap_or_else(|| "unknown".to_string()),
                schema_fingerprint,
                compatibility_generation,
            },
        });
    }
    Ok(result)
}

fn insert_snapshots(
    transaction: &Transaction<'_>,
    snapshots: &[WeeklySnapshot],
) -> Result<usize, StateError> {
    let mut inserted = 0;
    for snapshot in snapshots {
        inserted += transaction.execute(
            "INSERT OR IGNORE INTO snapshots (
                 source_file, byte_offset, observed_at, used_percent, resets_at,
                 window_minutes, plan_type
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                snapshot.id.source_file.to_string_lossy(),
                i64::try_from(snapshot.id.byte_offset).unwrap_or(i64::MAX),
                snapshot.observed_at.to_rfc3339(),
                snapshot.used_percent,
                snapshot.resets_at.map(|value| value.to_rfc3339()),
                snapshot.window_minutes,
                snapshot.plan_type,
            ],
        )?;
    }
    Ok(inserted)
}

struct ReplayOutcome {
    new_warnings: Vec<u32>,
}

type PreservedWindows =
    BTreeMap<DateTime<Utc>, (DateTime<Utc>, f64, String, CalibrationConfidence)>;

#[derive(Clone)]
struct ReplayWindow {
    started_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    latest_observed_at: DateTime<Utc>,
    latest_used_percent: f64,
    latest_resets_at: Option<DateTime<Utc>>,
    calibration: f64,
    calibration_id: String,
    calibration_confidence: CalibrationConfidence,
    weekly_points: f64,
    last_milestone: Option<u32>,
}

fn profile_for_snapshot(
    transaction: &Transaction<'_>,
    source_file: &str,
    byte_offset: i64,
    snapshot_plan: Option<&str>,
) -> Result<CalibrationProfile, StateError> {
    let identity: Option<CalibrationIdentity> = transaction
        .query_row(
            "SELECT COALESCE(plan_type, 'unknown'), COALESCE(model_slug, 'unknown'),
                    COALESCE(service_tier, 'unknown'), schema_fingerprint,
                    compatibility_generation
             FROM rate_limit_observations
             WHERE source_file = ?1 AND byte_offset = ?2",
            params![source_file, byte_offset],
            |row| {
                Ok(CalibrationIdentity {
                    plan_type: row.get(0)?,
                    model_slug: row.get(1)?,
                    service_tier: row.get(2)?,
                    schema_fingerprint: row.get(3)?,
                    compatibility_generation: row.get(4)?,
                })
            },
        )
        .optional()?;
    let identity = identity.unwrap_or_else(|| CalibrationIdentity {
        plan_type: snapshot_plan.unwrap_or("unknown").to_string(),
        model_slug: "unknown".to_string(),
        service_tier: "unknown".to_string(),
        schema_fingerprint: "legacy-weekly-snapshot-v1".to_string(),
        compatibility_generation: "legacy".to_string(),
    });
    select_calibration_profile(transaction, identity)
}

fn rebuild_windows(
    transaction: &Transaction<'_>,
    config: &TrackerConfig,
    now: DateTime<Utc>,
) -> Result<ReplayOutcome, StateError> {
    let preserved = load_preserved_windows(transaction)?;
    let mut statement = transaction.prepare(
        "SELECT id, source_file, byte_offset, observed_at, used_percent, resets_at, plan_type
         FROM snapshots ORDER BY observed_at, source_file, byte_offset",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, f64>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
        ))
    })?;
    let mut observations = Vec::new();
    for row in rows {
        let (id, source_file, byte_offset, observed_at, used_percent, resets_at, plan_type) = row?;
        observations.push((
            id,
            source_file,
            byte_offset,
            parse_timestamp(&observed_at)?,
            used_percent,
            resets_at.as_deref().map(parse_timestamp).transpose()?,
            plan_type,
        ));
    }
    drop(statement);

    let mut windows = Vec::<ReplayWindow>::new();
    let mut current: Option<ReplayWindow> = None;
    for (id, source_file, byte_offset, observed_at, used_percent, resets_at, plan_type) in
        observations
    {
        let starts_new = current
            .as_ref()
            .is_none_or(|window| observed_at >= window.ends_at);
        if starts_new {
            if let Some(window) = current.take() {
                windows.push(window);
            }
            let (ends_at, calibration, calibration_id, calibration_confidence) =
                if let Some(preserved) = preserved.get(&observed_at).cloned() {
                    preserved
                } else {
                    let profile = profile_for_snapshot(
                        transaction,
                        &source_file,
                        byte_offset,
                        plan_type.as_deref(),
                    )?;
                    (
                        observed_at + config.local_window_duration(),
                        profile.value.unwrap_or(config.calibration_weekly_points()),
                        profile.calibration_id,
                        profile.confidence,
                    )
                };
            current = Some(ReplayWindow {
                started_at: observed_at,
                ends_at,
                latest_observed_at: observed_at,
                latest_used_percent: used_percent,
                latest_resets_at: resets_at,
                calibration,
                calibration_id,
                calibration_confidence,
                weekly_points: 0.0,
                last_milestone: None,
            });
        } else if let Some(window) = current.as_mut() {
            let reset_changed = matches!(
                (window.latest_resets_at, resets_at),
                (Some(previous), Some(next)) if previous != next
            );
            let delta = if reset_changed || used_percent < window.latest_used_percent {
                used_percent
            } else {
                used_percent - window.latest_used_percent
            };
            window.weekly_points = (window.weekly_points + delta).max(0.0);
            window.latest_observed_at = observed_at;
            window.latest_used_percent = used_percent;
            window.latest_resets_at = resets_at;
            window.last_milestone =
                newest_milestone(window.weekly_points / window.calibration * 100.0, config);
        }
        transaction.execute("UPDATE snapshots SET affects_meter = 1 WHERE id = ?1", [id])?;
    }
    if let Some(window) = current {
        windows.push(window);
    }

    transaction.execute("DELETE FROM windows", [])?;
    let mut new_warnings = Vec::new();
    let final_index = windows.len().saturating_sub(1);
    for (index, window) in windows.iter().enumerate() {
        transaction.execute(
            "INSERT INTO windows (
                 started_at, ends_at, latest_observed_at, latest_used_percent,
                 calibration_weekly_points, accumulated_weekly_points,
                 last_emitted_milestone, lifecycle, calibration_id,
                 calibration_confidence
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                window.started_at.to_rfc3339(),
                window.ends_at.to_rfc3339(),
                window.latest_observed_at.to_rfc3339(),
                window.latest_used_percent,
                window.calibration,
                window.weekly_points,
                window.last_milestone,
                if index == final_index {
                    "current"
                } else {
                    "archived"
                },
                window.calibration_id,
                window.calibration_confidence.as_str(),
            ],
        )?;
        if let Some(milestone) = window.last_milestone {
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO emitted_warnings (
                     window_started_at, milestone, emitted_at
                 ) VALUES (?1, ?2, ?3)",
                params![window.started_at.to_rfc3339(), milestone, now.to_rfc3339()],
            )?;
            if inserted == 1 {
                new_warnings.push(milestone);
            }
        }
    }
    Ok(ReplayOutcome { new_warnings })
}

fn load_preserved_windows(transaction: &Transaction<'_>) -> Result<PreservedWindows, StateError> {
    let mut statement = transaction.prepare(
        "SELECT started_at, ends_at, calibration_weekly_points,
                    calibration_id, calibration_confidence FROM windows",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut result = BTreeMap::new();
    for row in rows {
        let (start, end, calibration, calibration_id, confidence) = row?;
        result.insert(
            parse_timestamp(&start)?,
            (
                parse_timestamp(&end)?,
                calibration,
                calibration_id,
                CalibrationConfidence::parse(&confidence),
            ),
        );
    }
    Ok(result)
}

fn newest_milestone(estimate: f64, config: &TrackerConfig) -> Option<u32> {
    let mut milestone = config
        .warning_thresholds()
        .iter()
        .copied()
        .filter(|threshold| f64::from(*threshold) <= estimate)
        .max();
    if estimate >= 100.0 {
        let super_milestone = 100
            + (((estimate.floor() as u32).saturating_sub(100)) / config.super_usage_step())
                * config.super_usage_step();
        if super_milestone > 100 {
            milestone = Some(milestone.map_or(super_milestone, |old| old.max(super_milestone)));
        }
    }
    milestone
}

fn read_display(
    transaction: &Transaction<'_>,
    config: &TrackerConfig,
    now: DateTime<Utc>,
) -> Result<DisplayCacheV1, StateError> {
    let calibration_kind = transaction
        .query_row(
            "SELECT calibration_kind FROM config_metadata WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|value| match value.as_str() {
            "measured" => CalibrationKind::Measured,
            "inherited_unvalidated" => CalibrationKind::InheritedUnvalidated,
            _ => CalibrationKind::Historical,
        })
        .unwrap_or(CalibrationKind::Historical);
    type CurrentRow = (String, String, String, f64, f64, f64, String, String);
    let row: Option<CurrentRow> = transaction
        .query_row(
            "SELECT started_at, ends_at, latest_observed_at, latest_used_percent,
                    calibration_weekly_points, accumulated_weekly_points,
                    calibration_id, calibration_confidence
             FROM windows WHERE lifecycle = 'current'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()?;
    let Some((
        start,
        end,
        observed,
        weekly_used,
        calibration,
        weekly_points,
        calibration_id,
        confidence,
    )) = row
    else {
        return Ok(unknown_display(
            now,
            None,
            None,
            CalibrationConfidence::Unsupported,
            calibration_kind,
        ));
    };
    let confidence = CalibrationConfidence::parse(&confidence);
    let start = parse_timestamp(&start)?;
    let end = parse_timestamp(&end)?;
    let observed = parse_timestamp(&observed)?;
    let unsupported_at: Option<String> = transaction
        .query_row(
            "SELECT observed_at FROM diagnostic_events
             WHERE observed_at IS NOT NULL AND (
                 code LIKE 'unsupported_%' OR code IN ('missing_rate_limits', 'missing_weekly_window')
             ) ORDER BY observed_at DESC, rowid DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if unsupported_at
        .as_deref()
        .map(parse_timestamp)
        .transpose()?
        .is_some_and(|diagnostic_at| diagnostic_at >= observed)
    {
        return Ok(unknown_display(
            now,
            Some(calibration),
            Some(calibration_id),
            confidence,
            calibration_kind,
        ));
    }
    if confidence == CalibrationConfidence::Unsupported {
        return Ok(unknown_display(
            now,
            None,
            Some(calibration_id),
            confidence,
            calibration_kind,
        ));
    }
    let age = (now - observed).num_seconds().max(0);
    let stale = now >= end || now - observed > config.stale_after();
    let local_estimate = weekly_points / calibration * 100.0;
    let server_five_hour: Option<(f64, String)> = transaction
        .query_row(
            "SELECT w.used_percent, o.observed_at
             FROM observed_rate_limit_windows w
             JOIN rate_limit_observations o
               ON o.source_file = w.source_file AND o.byte_offset = w.byte_offset
             WHERE w.window_kind = 'five_hour'
             ORDER BY o.observed_at DESC, o.source_file DESC, o.byte_offset DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let server_five_hour = server_five_hour
        .map(|(value, at)| -> Result<_, StateError> { Ok((value, parse_timestamp(&at)?)) })
        .transpose()?
        .filter(|(_, at)| now - *at <= config.stale_after());
    let (five_hour_value, five_hour_value_source) = server_five_hour
        .map(|(value, _)| (value, "real_server_five_hour".to_string()))
        .unwrap_or((local_estimate, "local_calibrated_estimate".to_string()));
    Ok(DisplayCacheV1 {
        schema_version: DISPLAY_SCHEMA_VERSION,
        generated_at: now,
        status: if stale {
            WindowStatus::Stale
        } else {
            WindowStatus::Fresh
        },
        stale,
        data_age_seconds: Some(age),
        observed_at: Some(observed),
        window_started_at: Some(start),
        window_ends_at: Some(end),
        weekly_points: Some(weekly_points),
        five_hour_estimate_percent: Some(five_hour_value),
        five_hour_estimate_left_percent: Some((100.0 - five_hour_value).max(0.0)),
        five_hour_value_source: Some(five_hour_value_source),
        weekly_limit_used_percent: Some(weekly_used),
        weekly_limit_left_percent: Some((100.0 - weekly_used).max(0.0)),
        calibration_weekly_points: Some(calibration),
        calibration_kind,
        calibration_id: Some(calibration_id),
        calibration_confidence: Some(confidence),
    })
}

fn unknown_display(
    now: DateTime<Utc>,
    calibration: Option<f64>,
    calibration_id: Option<String>,
    confidence: CalibrationConfidence,
    calibration_kind: CalibrationKind,
) -> DisplayCacheV1 {
    DisplayCacheV1 {
        schema_version: DISPLAY_SCHEMA_VERSION,
        generated_at: now,
        status: WindowStatus::Unknown,
        stale: true,
        data_age_seconds: None,
        observed_at: None,
        window_started_at: None,
        window_ends_at: None,
        weekly_points: None,
        five_hour_estimate_percent: None,
        five_hour_estimate_left_percent: None,
        five_hour_value_source: None,
        weekly_limit_used_percent: None,
        weekly_limit_left_percent: None,
        calibration_weekly_points: calibration,
        calibration_kind,
        calibration_id,
        calibration_confidence: Some(confidence),
    }
}

fn write_display_atomic(paths: &StatePaths, display: &DisplayCacheV1) -> Result<(), StateError> {
    write_json_atomic(&paths.directory, &paths.display, display)
}

fn write_json_atomic<T: Serialize>(
    directory: &Path,
    destination: &Path,
    value: &T,
) -> Result<(), StateError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut temporary = NamedTempFile::new_in(directory).map_err(|source| StateError::Io {
        path: destination.to_path_buf(),
        source,
    })?;
    temporary
        .write_all(&bytes)
        .and_then(|()| temporary.write_all(b"\n"))
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| StateError::Io {
            path: destination.to_path_buf(),
            source,
        })?;
    temporary
        .persist(destination)
        .map_err(|error| StateError::Io {
            path: destination.to_path_buf(),
            source: error.error,
        })?;
    if let Ok(directory_file) = fs::File::open(directory) {
        let _ = directory_file.sync_all();
    }
    Ok(())
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, StateError> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| StateError::InvalidTimestamp(value.to_owned()))
}
