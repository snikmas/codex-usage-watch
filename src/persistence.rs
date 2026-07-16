use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::calibration::CalibrationConfidence;
use crate::ingest::{IngestBatch, IngestError};
use crate::model::{DomainError, TrackerConfig, WeeklySnapshot, WindowStatus};
use crate::private_fs::{
    ensure_private_directory, ensure_private_file, ensure_private_file_if_exists,
    write_private_atomic,
};

mod backups;
mod calibration_state;
mod compatibility_state;
mod display;
mod migrations;
mod transcripts;
mod window_replay;

use window_replay::{insert_snapshots, rebuild_windows};

const SCHEMA_VERSION: i64 = 10;
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const HOOK_BUSY_TIMEOUT: Duration = Duration::from_millis(250);

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
        Self::open_at(paths, config, BUSY_TIMEOUT)
    }

    /// Open state with a deliberately short lock budget for a Codex lifecycle
    /// hook. Hooks are advisory and must fail open before Codex's own timeout;
    /// interactive CLI commands retain the longer normal retry budget.
    pub fn open_for_hook(config: TrackerConfig) -> Result<Self, StateError> {
        let paths = StatePaths::resolve(&config)?;
        Self::open_at(paths, config, HOOK_BUSY_TIMEOUT)
    }

    pub fn open_in(
        directory: impl Into<PathBuf>,
        config: TrackerConfig,
    ) -> Result<Self, StateError> {
        Self::open_at(StatePaths::in_directory(directory), config, BUSY_TIMEOUT)
    }

    fn open_at(
        paths: StatePaths,
        config: TrackerConfig,
        busy_timeout: Duration,
    ) -> Result<Self, StateError> {
        ensure_private_directory(&paths.directory).map_err(|source| StateError::Io {
            path: paths.directory.clone(),
            source,
        })?;
        repair_tracker_permissions(&paths)?;
        let mut connection = Connection::open(&paths.database)?;
        ensure_private_file(&paths.database).map_err(|source| StateError::Io {
            path: paths.database.clone(),
            source,
        })?;
        connection.busy_timeout(busy_timeout)?;
        retry_busy(busy_timeout, || {
            connection
                .pragma_update(None, "journal_mode", "WAL")
                .map_err(StateError::from)
        })?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        retry_busy(busy_timeout, || migrations::migrate(&mut connection))?;
        retry_busy(busy_timeout, || {
            migrations::write_config_metadata(&connection, &config)
        })?;
        repair_tracker_permissions(&paths)?;
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
}

fn retry_busy<T>(
    timeout: Duration,
    mut operation: impl FnMut() -> Result<T, StateError>,
) -> Result<T, StateError> {
    let deadline = Instant::now() + timeout;
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

fn write_json_atomic<T: Serialize>(
    directory: &Path,
    destination: &Path,
    value: &T,
) -> Result<(), StateError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut encoded = bytes;
    encoded.push(b'\n');
    write_private_atomic(directory, destination, &encoded).map_err(|source| StateError::Io {
        path: destination.to_path_buf(),
        source,
    })
}

fn repair_tracker_permissions(paths: &StatePaths) -> Result<(), StateError> {
    let database_wal = paths.directory.join("state.sqlite3-wal");
    let database_shm = paths.directory.join("state.sqlite3-shm");
    let release_metadata = paths.directory.join("release-metadata.json");
    for path in [
        &paths.database,
        &database_wal,
        &database_shm,
        &paths.display,
        &paths.calibration_report,
        &release_metadata,
    ] {
        ensure_private_file_if_exists(path).map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, StateError> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| StateError::InvalidTimestamp(value.to_owned()))
}
