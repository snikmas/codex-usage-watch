use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Transaction, params};

use super::calibration_state::select_calibration_profile;
use super::{StateError, parse_timestamp};
use crate::calculate::{WeeklyTransition, weekly_transition};
use crate::calibration::{CalibrationConfidence, CalibrationIdentity, CalibrationProfile};
use crate::model::{
    FIVE_HOUR_WINDOW_MINUTES, ObservationId, ObservedRateLimitWindow, TrackerConfig,
    UsageObservation, WEEKLY_WINDOW_MINUTES, WeeklySnapshot,
};
use crate::reset::{ResetClassification, classify_reset, inferred_epoch_start, same_epoch};

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
struct ReplayItem {
    snapshot_id: i64,
    source_file: String,
    byte_offset: i64,
    observation: UsageObservation,
}

#[derive(Clone)]
struct ReplayWindow {
    started_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    latest_observed_at: DateTime<Utc>,
    latest_used_percent: f64,
    latest_resets_at: Option<DateTime<Utc>>,
    latest_source_file: String,
    latest_byte_offset: i64,
    server_five_hour_percent: Option<f64>,
    server_five_hour_observed_at: Option<DateTime<Utc>>,
    calibration: f64,
    calibration_id: String,
    calibration_confidence: CalibrationConfidence,
    weekly_points: f64,
    last_milestone: Option<u32>,
    boundary_kind: String,
    boundary_at: DateTime<Utc>,
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
    let mut observations = load_replay_items(transaction)?;
    backfill_known_five_hour_epochs(&mut observations);
    let mut windows = Vec::<ReplayWindow>::new();
    let mut current: Option<ReplayWindow> = None;
    let mut previous_accepted: Option<UsageObservation> = None;
    transaction.execute("UPDATE snapshots SET affects_meter = 0", [])?;
    transaction.execute("DELETE FROM reset_events", [])?;

    for item in observations {
        let decision = previous_accepted
            .as_ref()
            .map(|previous| classify_reset(previous, &item.observation));
        if decision
            .as_ref()
            .is_some_and(|value| value.classification == ResetClassification::OldEpoch)
        {
            persist_reset_event(
                transaction,
                previous_accepted.as_ref(),
                &item,
                decision.as_ref().unwrap(),
            )?;
            continue;
        }

        let five_changed = previous_accepted.as_ref().is_some_and(|previous| {
            !same_epoch(
                previous
                    .five_hour
                    .as_ref()
                    .and_then(|window| window.resets_at),
                item.observation
                    .five_hour
                    .as_ref()
                    .and_then(|window| window.resets_at),
            ) && previous.five_hour.is_some()
                && item.observation.five_hour.is_some()
        });
        let server_split = five_changed
            && decision.as_ref().is_some_and(|value| {
                matches!(
                    value.classification,
                    ResetClassification::NaturalFiveHour
                        | ResetClassification::InferredFull
                        | ResetClassification::Ambiguous
                )
            });
        let local_expiry = current
            .as_ref()
            .is_some_and(|window| !server_split && item.observation.observed_at >= window.ends_at);

        if current.is_none() || server_split || local_expiry {
            if let Some(mut old) = current.take() {
                if server_split {
                    if let Some(boundary) = decision.as_ref().and_then(|value| value.boundary_at) {
                        old.ends_at = boundary.max(old.started_at).min(old.ends_at);
                    }
                }
                windows.push(old);
            }
            let boundary_kind = decision
                .as_ref()
                .filter(|_| server_split)
                .map(|value| value.classification.as_str())
                .unwrap_or(if local_expiry {
                    "local_expiry"
                } else if item.observation.five_hour.is_some() {
                    "server_epoch"
                } else {
                    "initial_observation"
                });
            let boundary_at = decision
                .as_ref()
                .filter(|_| server_split)
                .and_then(|value| value.boundary_at)
                .or_else(|| server_epoch_bounds(&item.observation).map(|(start, _)| start))
                .unwrap_or(item.observation.observed_at);
            let seed = if server_split {
                seed_after_five_hour_boundary(previous_accepted.as_ref(), &item.observation)
            } else {
                0.0
            };
            current = Some(new_replay_window(
                transaction,
                config,
                &preserved,
                &item,
                boundary_kind,
                boundary_at,
                seed,
            )?);
            transaction.execute(
                "UPDATE snapshots SET affects_meter = 1 WHERE id = ?1",
                [item.snapshot_id],
            )?;
        } else if let Some(window) = current.as_mut() {
            let delta = match weekly_transition(
                window.latest_used_percent,
                window.latest_resets_at,
                item.observation
                    .weekly
                    .as_ref()
                    .map_or(window.latest_used_percent, |value| value.used_percent),
                item.observation
                    .weekly
                    .as_ref()
                    .and_then(|value| value.resets_at),
            ) {
                WeeklyTransition::Advance(delta) | WeeklyTransition::Reset(delta) => delta,
                WeeklyTransition::IgnoreRegression => {
                    if let Some(decision) = decision.as_ref() {
                        if decision.classification != ResetClassification::NoReset {
                            persist_reset_event(
                                transaction,
                                previous_accepted.as_ref(),
                                &item,
                                decision,
                            )?;
                        }
                    }
                    continue;
                }
            };
            window.weekly_points = (window.weekly_points + delta).max(0.0);
            update_latest(window, &item);
            window.last_milestone =
                newest_milestone(window.weekly_points / window.calibration * 100.0, config);
            transaction.execute(
                "UPDATE snapshots SET affects_meter = 1 WHERE id = ?1",
                [item.snapshot_id],
            )?;
        }

        if let Some(decision) = decision.as_ref() {
            if decision.classification != ResetClassification::NoReset {
                persist_reset_event(transaction, previous_accepted.as_ref(), &item, decision)?;
            }
        }
        previous_accepted = Some(item.observation);
    }
    if let Some(window) = current {
        windows.push(window);
    }

    transaction.execute("DELETE FROM windows", [])?;
    let mut new_warnings = Vec::new();
    let final_index = windows.len().saturating_sub(1);
    let latest_manual_reset: Option<DateTime<Utc>> = transaction
        .query_row(
            "SELECT occurred_at FROM control_events WHERE event_type = 'manual_reset'
             ORDER BY occurred_at DESC, id DESC LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .as_deref()
        .map(parse_timestamp)
        .transpose()?;
    for (index, window) in windows.iter().enumerate() {
        let is_current = index == final_index
            && latest_manual_reset.is_none_or(|reset_at| window.latest_observed_at > reset_at);
        transaction.execute(
            "INSERT INTO windows (
                 started_at, ends_at, latest_observed_at, latest_used_percent,
                 calibration_weekly_points, accumulated_weekly_points,
                 last_emitted_milestone, lifecycle, calibration_id,
                 calibration_confidence, boundary_kind, boundary_at,
                 latest_source_file, latest_byte_offset, server_five_hour_percent,
                 server_five_hour_observed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                window.started_at.to_rfc3339(),
                window.ends_at.to_rfc3339(),
                window.latest_observed_at.to_rfc3339(),
                window.latest_used_percent,
                window.calibration,
                window.weekly_points,
                window.last_milestone,
                if is_current { "current" } else { "archived" },
                window.calibration_id,
                window.calibration_confidence.as_str(),
                window.boundary_kind,
                window.boundary_at.to_rfc3339(),
                window.latest_source_file,
                window.latest_byte_offset,
                window.server_five_hour_percent,
                window
                    .server_five_hour_observed_at
                    .map(|value| value.to_rfc3339()),
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

fn load_replay_items(transaction: &Transaction<'_>) -> Result<Vec<ReplayItem>, StateError> {
    type Row = (
        i64,
        String,
        i64,
        String,
        f64,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<f64>,
        Option<i64>,
        Option<String>,
        Option<f64>,
        Option<i64>,
        Option<String>,
    );
    let mut statement = transaction.prepare(
        "SELECT s.id, s.source_file, s.byte_offset, s.observed_at, s.used_percent,
                s.resets_at, s.plan_type, o.model_slug, o.codex_version,
                o.service_tier, o.plan_type, o.schema_fingerprint,
                f.used_percent, f.window_minutes, f.resets_at,
                w.used_percent, w.window_minutes, w.resets_at
         FROM snapshots s
         LEFT JOIN rate_limit_observations o
           ON o.source_file = s.source_file AND o.byte_offset = s.byte_offset
         LEFT JOIN observed_rate_limit_windows f
           ON f.source_file = s.source_file AND f.byte_offset = s.byte_offset
          AND f.window_kind = 'five_hour'
         LEFT JOIN observed_rate_limit_windows w
           ON w.source_file = s.source_file AND w.byte_offset = s.byte_offset
          AND w.window_kind = 'weekly'
         ORDER BY s.observed_at, s.source_file, s.byte_offset",
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
            row.get(12)?,
            row.get(13)?,
            row.get(14)?,
            row.get(15)?,
            row.get(16)?,
            row.get(17)?,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (
            id,
            source,
            offset,
            observed,
            weekly_used,
            snapshot_weekly_reset,
            snapshot_plan,
            model,
            codex_version,
            tier,
            observation_plan,
            fingerprint,
            five_used,
            five_minutes,
            five_reset,
            stored_weekly_used,
            weekly_minutes,
            stored_weekly_reset,
        ): Row = row?;
        let weekly = Some(ObservedRateLimitWindow {
            used_percent: stored_weekly_used.unwrap_or(weekly_used),
            window_minutes: weekly_minutes
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(WEEKLY_WINDOW_MINUTES),
            resets_at: stored_weekly_reset
                .as_deref()
                .or(snapshot_weekly_reset.as_deref())
                .map(parse_timestamp)
                .transpose()?,
        });
        let five_hour = five_used
            .map(|used_percent| -> Result<_, StateError> {
                Ok(ObservedRateLimitWindow {
                    used_percent,
                    window_minutes: five_minutes
                        .and_then(|value| u32::try_from(value).ok())
                        .unwrap_or(FIVE_HOUR_WINDOW_MINUTES),
                    resets_at: five_reset.as_deref().map(parse_timestamp).transpose()?,
                })
            })
            .transpose()?;
        result.push(ReplayItem {
            snapshot_id: id,
            source_file: source.clone(),
            byte_offset: offset,
            observation: UsageObservation {
                id: ObservationId::new(PathBuf::from(source), u64::try_from(offset).unwrap_or(0)),
                observed_at: parse_timestamp(&observed)?,
                five_hour,
                weekly,
                model_slug: model,
                codex_version,
                service_tier: tier,
                plan_type: observation_plan.or(snapshot_plan),
                schema_fingerprint: fingerprint
                    .unwrap_or_else(|| "legacy-weekly-snapshot-v1".to_string()),
            },
        });
    }
    Ok(result)
}

/// A transcript may expose only the weekly window at first and include the
/// paired 300-minute window in a later snapshot. On a full replay, safely apply
/// that later epoch metadata to earlier observations that fall inside the same
/// server interval. This changes no stored raw observation and never crosses a
/// known epoch boundary.
fn backfill_known_five_hour_epochs(items: &mut [ReplayItem]) {
    let mut next_known: Option<ObservedRateLimitWindow> = None;
    for item in items.iter_mut().rev() {
        if let Some(five_hour) = item.observation.five_hour.as_ref() {
            next_known = Some(five_hour.clone());
            continue;
        }
        let Some(known) = next_known.as_ref() else {
            continue;
        };
        let Some(start) = inferred_epoch_start(known) else {
            next_known = None;
            continue;
        };
        let Some(end) = known.resets_at else {
            next_known = None;
            continue;
        };
        if item.observation.observed_at >= start - crate::reset::RESET_TIMESTAMP_JITTER
            && item.observation.observed_at <= end + crate::reset::RESET_TIMESTAMP_JITTER
        {
            item.observation.five_hour = Some(known.clone());
        } else {
            next_known = None;
        }
    }
}

fn new_replay_window(
    transaction: &Transaction<'_>,
    config: &TrackerConfig,
    preserved: &PreservedWindows,
    item: &ReplayItem,
    boundary_kind: &str,
    boundary_at: DateTime<Utc>,
    weekly_points: f64,
) -> Result<ReplayWindow, StateError> {
    let (server_start, server_end) = if boundary_kind == "local_expiry" {
        (
            item.observation.observed_at,
            item.observation.observed_at + config.local_window_duration(),
        )
    } else {
        server_epoch_bounds(&item.observation).unwrap_or((
            item.observation.observed_at,
            item.observation.observed_at + config.local_window_duration(),
        ))
    };
    let (ends_at, calibration, calibration_id, calibration_confidence) =
        if let Some(preserved) = preserved.get(&server_start).cloned() {
            preserved
        } else {
            let profile = profile_for_snapshot(
                transaction,
                &item.source_file,
                item.byte_offset,
                item.observation.plan_type.as_deref(),
            )?;
            (
                server_end,
                profile.value.unwrap_or(config.calibration_weekly_points()),
                profile.calibration_id,
                profile.confidence,
            )
        };
    Ok(ReplayWindow {
        started_at: server_start,
        ends_at,
        latest_observed_at: item.observation.observed_at,
        latest_used_percent: item
            .observation
            .weekly
            .as_ref()
            .map_or(0.0, |value| value.used_percent),
        latest_resets_at: item
            .observation
            .weekly
            .as_ref()
            .and_then(|value| value.resets_at),
        latest_source_file: item.source_file.clone(),
        latest_byte_offset: item.byte_offset,
        server_five_hour_percent: item
            .observation
            .five_hour
            .as_ref()
            .map(|value| value.used_percent),
        server_five_hour_observed_at: item
            .observation
            .five_hour
            .as_ref()
            .map(|_| item.observation.observed_at),
        calibration,
        calibration_id,
        calibration_confidence,
        weekly_points: weekly_points.max(0.0),
        last_milestone: newest_milestone(weekly_points / calibration * 100.0, config),
        boundary_kind: boundary_kind.to_string(),
        boundary_at,
    })
}

fn server_epoch_bounds(observation: &UsageObservation) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let five = observation.five_hour.as_ref()?;
    if five.window_minutes != FIVE_HOUR_WINDOW_MINUTES {
        return None;
    }
    let start = inferred_epoch_start(five)?;
    let end = five.resets_at?;
    (observation.observed_at >= start - crate::reset::RESET_TIMESTAMP_JITTER
        && observation.observed_at <= end + crate::reset::RESET_TIMESTAMP_JITTER)
        .then_some((start, end))
}

fn seed_after_five_hour_boundary(
    previous: Option<&UsageObservation>,
    current: &UsageObservation,
) -> f64 {
    let Some(current_weekly) = current.weekly.as_ref() else {
        return 0.0;
    };
    let Some(previous_weekly) = previous.and_then(|value| value.weekly.as_ref()) else {
        return current_weekly.used_percent;
    };
    if same_epoch(previous_weekly.resets_at, current_weekly.resets_at) {
        (current_weekly.used_percent - previous_weekly.used_percent).max(0.0)
    } else {
        current_weekly.used_percent
    }
}

fn update_latest(window: &mut ReplayWindow, item: &ReplayItem) {
    window.latest_observed_at = item.observation.observed_at;
    if let Some(weekly) = item.observation.weekly.as_ref() {
        window.latest_used_percent = weekly.used_percent;
        window.latest_resets_at = weekly.resets_at;
    }
    window.latest_source_file = item.source_file.clone();
    window.latest_byte_offset = item.byte_offset;
    if let Some(five_hour) = item.observation.five_hour.as_ref() {
        window.server_five_hour_percent = Some(five_hour.used_percent);
        window.server_five_hour_observed_at = Some(item.observation.observed_at);
    }
}

fn persist_reset_event(
    transaction: &Transaction<'_>,
    previous: Option<&UsageObservation>,
    item: &ReplayItem,
    decision: &crate::reset::ResetDecision,
) -> Result<(), StateError> {
    if decision.classification == ResetClassification::NoReset {
        return Ok(());
    }
    transaction.execute(
        "INSERT OR REPLACE INTO reset_events (
             source_file, byte_offset, observed_at, boundary_at, classification,
             confidence, reason, previous_five_hour_resets_at, new_five_hour_resets_at,
             previous_weekly_resets_at, new_weekly_resets_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            item.source_file,
            item.byte_offset,
            item.observation.observed_at.to_rfc3339(),
            decision.boundary_at.map(|value| value.to_rfc3339()),
            decision.classification.as_str(),
            if matches!(
                decision.classification,
                ResetClassification::InferredFull | ResetClassification::Ambiguous
            ) {
                "inferred"
            } else {
                "observed_epoch"
            },
            decision.reason,
            previous
                .and_then(|value| value.five_hour.as_ref())
                .and_then(|value| value.resets_at)
                .map(|value| value.to_rfc3339()),
            item.observation
                .five_hour
                .as_ref()
                .and_then(|value| value.resets_at)
                .map(|value| value.to_rfc3339()),
            previous
                .and_then(|value| value.weekly.as_ref())
                .and_then(|value| value.resets_at)
                .map(|value| value.to_rfc3339()),
            item.observation
                .weekly
                .as_ref()
                .and_then(|value| value.resets_at)
                .map(|value| value.to_rfc3339()),
        ],
    )?;
    Ok(())
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
