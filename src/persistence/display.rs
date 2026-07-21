use std::fs;
use std::io;

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};

use super::{
    CalibrationKind, DisplayCacheV1, StateError, StatePaths, StateStore, parse_timestamp,
    write_json_atomic,
};
use crate::calibration::CalibrationConfidence;
use crate::model::{TrackerConfig, WindowStatus};

const DISPLAY_SCHEMA_VERSION: u32 = 2;

impl StateStore {
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

    pub fn load_display_read_only(
        config: &TrackerConfig,
        now: DateTime<Utc>,
    ) -> Result<DisplayCacheV1, StateError> {
        let paths = StatePaths::resolve(config)?;
        match fs::read(&paths.display) {
            Ok(bytes) => match serde_json::from_slice::<DisplayCacheV1>(&bytes) {
                Ok(display) if display.schema_version == DISPLAY_SCHEMA_VERSION => {
                    Ok(refresh_freshness(display, config, now))
                }
                Ok(_) | Err(_) => Ok(unknown_display(
                    now,
                    Some(config.calibration_weekly_points()),
                    None,
                    CalibrationConfidence::Unsupported,
                    CalibrationKind::Historical,
                )),
            },
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(unknown_display(
                now,
                Some(config.calibration_weekly_points()),
                None,
                CalibrationConfidence::Unsupported,
                CalibrationKind::Historical,
            )),
            Err(source) => Err(StateError::Io {
                path: paths.display,
                source,
            }),
        }
    }

    pub(super) fn regenerate_display(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<DisplayCacheV1, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let display = read_display(&transaction, &self.config, now)?;
        write_display_atomic(&self.paths, &display)?;
        transaction.commit()?;
        Ok(display)
    }
}

fn refresh_freshness(
    mut display: DisplayCacheV1,
    config: &TrackerConfig,
    now: DateTime<Utc>,
) -> DisplayCacheV1 {
    if display.status == WindowStatus::Unknown {
        return display;
    }
    let Some(observed_at) = display.observed_at else {
        display.status = WindowStatus::Unknown;
        display.stale = true;
        display.data_age_seconds = None;
        return display;
    };
    let age = (now - observed_at).num_seconds().max(0);
    let expired = display.window_ends_at.is_some_and(|ends_at| now >= ends_at);
    let stale = expired || now - observed_at > config.stale_after();
    display.status = if stale {
        WindowStatus::Stale
    } else {
        WindowStatus::Fresh
    };
    display.stale = stale;
    display.data_age_seconds = Some(age);
    display
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
    type CurrentRow = (
        String,
        String,
        String,
        f64,
        f64,
        f64,
        String,
        String,
        String,
        String,
        Option<f64>,
    );
    let row: Option<CurrentRow> = transaction
        .query_row(
            "SELECT started_at, ends_at, latest_observed_at, latest_used_percent,
                    calibration_weekly_points, accumulated_weekly_points,
                    calibration_id, calibration_confidence, boundary_kind,
                    boundary_at, server_five_hour_percent
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
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
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
        boundary_kind,
        boundary_at,
        server_five_hour_percent,
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
    let fresh_server_five_hour =
        server_five_hour_percent.filter(|_| now - observed <= config.stale_after());
    let (five_hour_value, five_hour_value_source) = fresh_server_five_hour
        .map(|value| (value, "real_server_five_hour".to_string()))
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
        window_boundary_kind: Some(boundary_kind),
        window_boundary_at: Some(parse_timestamp(&boundary_at)?),
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
        window_boundary_kind: None,
        window_boundary_at: None,
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
