use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Transaction, params};

use super::calibration_state::select_calibration_profile;
use super::{StateError, parse_timestamp};
use crate::calibration::{CalibrationConfidence, CalibrationIdentity, CalibrationProfile};
use crate::model::{TrackerConfig, WeeklySnapshot};

pub(super) fn insert_snapshots(
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

pub(super) struct ReplayOutcome {
    pub(super) new_warnings: Vec<u32>,
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

pub(super) fn rebuild_windows(
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
