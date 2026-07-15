use std::path::PathBuf;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const WEEKLY_WINDOW_MINUTES: u32 = 10_080;
pub const FIVE_HOUR_WINDOW_MINUTES: u32 = 300;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum DomainError {
    #[error("calibration must be finite and greater than zero")]
    InvalidCalibration,
    #[error("local window duration must be greater than zero")]
    InvalidWindowDuration,
    #[error("stale threshold must be greater than zero")]
    InvalidStaleThreshold,
    #[error("super-usage milestone step must be greater than zero")]
    InvalidSuperUsageStep,
    #[error("CODEX_USAGE_WATCH_THRESHOLDS must be a comma-separated list of positive integers")]
    InvalidWarningThresholds,
    #[error("weekly usage must be finite and between 0 and 100")]
    InvalidWeeklyUsage,
    #[error("a WeeklySnapshot must represent a 10080-minute window")]
    NotWeeklyWindow,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackerConfig {
    calibration_weekly_points: f64,
    local_window_duration: TimeDelta,
    stale_after: TimeDelta,
    warning_thresholds: Vec<u32>,
    super_usage_step: u32,
    state_dir_override: Option<PathBuf>,
}

impl TrackerConfig {
    /// Load the supported version 1 user settings from the environment.
    ///
    /// Keeping this surface deliberately small makes hook, CLI, and installer
    /// behavior identical without introducing a second mutable config file.
    pub fn from_env() -> Result<Self, DomainError> {
        let mut config = Self::default();
        let Some(raw) = std::env::var_os("CODEX_USAGE_WATCH_THRESHOLDS") else {
            return Ok(config);
        };
        let raw = raw
            .into_string()
            .map_err(|_| DomainError::InvalidWarningThresholds)?;
        let thresholds = raw
            .split(',')
            .map(str::trim)
            .map(|value| {
                value
                    .parse::<u32>()
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or(DomainError::InvalidWarningThresholds)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if thresholds.is_empty() {
            return Err(DomainError::InvalidWarningThresholds);
        }
        config.warning_thresholds = thresholds;
        config.warning_thresholds.sort_unstable();
        config.warning_thresholds.dedup();
        Ok(config)
    }

    pub fn new(
        calibration_weekly_points: f64,
        local_window_duration: TimeDelta,
        stale_after: TimeDelta,
        mut warning_thresholds: Vec<u32>,
        super_usage_step: u32,
        state_dir_override: Option<PathBuf>,
    ) -> Result<Self, DomainError> {
        if !calibration_weekly_points.is_finite() || calibration_weekly_points <= 0.0 {
            return Err(DomainError::InvalidCalibration);
        }
        if local_window_duration <= TimeDelta::zero() {
            return Err(DomainError::InvalidWindowDuration);
        }
        if stale_after <= TimeDelta::zero() {
            return Err(DomainError::InvalidStaleThreshold);
        }
        if super_usage_step == 0 {
            return Err(DomainError::InvalidSuperUsageStep);
        }

        warning_thresholds.sort_unstable();
        warning_thresholds.dedup();

        Ok(Self {
            calibration_weekly_points,
            local_window_duration,
            stale_after,
            warning_thresholds,
            super_usage_step,
            state_dir_override,
        })
    }

    pub fn calibration_weekly_points(&self) -> f64 {
        self.calibration_weekly_points
    }

    pub fn local_window_duration(&self) -> TimeDelta {
        self.local_window_duration
    }

    pub fn stale_after(&self) -> TimeDelta {
        self.stale_after
    }

    pub fn warning_thresholds(&self) -> &[u32] {
        &self.warning_thresholds
    }

    pub fn super_usage_step(&self) -> u32 {
        self.super_usage_step
    }

    pub fn state_dir_override(&self) -> Option<&std::path::Path> {
        self.state_dir_override.as_deref()
    }

    pub fn with_calibration_weekly_points(
        &self,
        calibration_weekly_points: f64,
    ) -> Result<Self, DomainError> {
        Self::new(
            calibration_weekly_points,
            self.local_window_duration,
            self.stale_after,
            self.warning_thresholds.clone(),
            self.super_usage_step,
            self.state_dir_override.clone(),
        )
    }
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self::new(
            15.8,
            TimeDelta::hours(5),
            TimeDelta::minutes(15),
            vec![75, 90, 100],
            10,
            None,
        )
        .expect("the built-in tracker configuration is valid")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObservationId {
    pub source_file: PathBuf,
    pub byte_offset: u64,
}

impl ObservationId {
    pub fn new(source_file: impl Into<PathBuf>, byte_offset: u64) -> Self {
        Self {
            source_file: source_file.into(),
            byte_offset,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeeklySnapshot {
    pub id: ObservationId,
    pub observed_at: DateTime<Utc>,
    pub used_percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
    pub window_minutes: u32,
    pub plan_type: Option<String>,
}

impl WeeklySnapshot {
    pub fn new(
        id: ObservationId,
        observed_at: DateTime<Utc>,
        used_percent: f64,
        resets_at: Option<DateTime<Utc>>,
        window_minutes: u32,
        plan_type: Option<String>,
    ) -> Result<Self, DomainError> {
        if !used_percent.is_finite() || !(0.0..=100.0).contains(&used_percent) {
            return Err(DomainError::InvalidWeeklyUsage);
        }
        if window_minutes != WEEKLY_WINDOW_MINUTES {
            return Err(DomainError::NotWeeklyWindow);
        }

        Ok(Self {
            id,
            observed_at,
            used_percent,
            resets_at,
            window_minutes,
            plan_type,
        })
    }
}

/// One validated server rate-limit window. Missing or malformed windows are
/// represented by diagnostics instead of synthesized zero-valued readings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservedRateLimitWindow {
    pub used_percent: f64,
    pub window_minutes: u32,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageObservation {
    pub id: ObservationId,
    pub observed_at: DateTime<Utc>,
    pub five_hour: Option<ObservedRateLimitWindow>,
    pub weekly: Option<ObservedRateLimitWindow>,
    pub model_slug: Option<String>,
    pub codex_version: Option<String>,
    pub service_tier: Option<String>,
    pub plan_type: Option<String>,
    pub schema_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestDiagnostic {
    pub id: ObservationId,
    pub observed_at: Option<DateTime<Utc>>,
    pub code: String,
    pub field: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowStatus {
    Fresh,
    Stale,
    Expired,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalWindow {
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) ends_at: DateTime<Utc>,
    pub(crate) calibration_weekly_points: f64,
    pub(crate) latest_snapshot: WeeklySnapshot,
    pub(crate) accumulated_weekly_points: f64,
    pub(crate) last_emitted_milestone: Option<u32>,
}

impl LocalWindow {
    pub fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }

    pub fn ends_at(&self) -> DateTime<Utc> {
        self.ends_at
    }

    pub fn calibration_weekly_points(&self) -> f64 {
        self.calibration_weekly_points
    }

    pub fn latest_snapshot(&self) -> &WeeklySnapshot {
        &self.latest_snapshot
    }

    pub fn accumulated_weekly_points(&self) -> f64 {
        self.accumulated_weekly_points
    }

    pub fn last_emitted_milestone(&self) -> Option<u32> {
        self.last_emitted_milestone
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeterReading {
    pub window_started_at: DateTime<Utc>,
    pub window_ends_at: DateTime<Utc>,
    pub weekly_points: f64,
    pub five_hour_estimate_percent: f64,
    pub status: WindowStatus,
    pub observed_at: DateTime<Utc>,
    pub crossed_milestone: Option<u32>,
}
