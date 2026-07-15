use std::collections::HashSet;

use chrono::{DateTime, Utc};

use crate::model::{
    LocalWindow, MeterReading, ObservationId, TrackerConfig, WeeklySnapshot, WindowStatus,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ApplyOutcome {
    pub accepted: bool,
    pub reading: MeterReading,
    pub archived_reading: Option<MeterReading>,
}

#[derive(Debug, Clone)]
pub struct AccountingEngine {
    config: TrackerConfig,
    window: Option<LocalWindow>,
    seen: HashSet<ObservationId>,
}

impl AccountingEngine {
    pub fn new(config: TrackerConfig) -> Self {
        Self {
            config,
            window: None,
            seen: HashSet::new(),
        }
    }

    pub fn window(&self) -> Option<&LocalWindow> {
        self.window.as_ref()
    }

    pub fn reading(&self, now: DateTime<Utc>) -> Option<MeterReading> {
        self.window
            .as_ref()
            .map(|window| reading_for(window, &self.config, now, None))
    }

    pub fn apply(&mut self, snapshot: WeeklySnapshot, now: DateTime<Utc>) -> ApplyOutcome {
        if self.seen.contains(&snapshot.id) {
            return ApplyOutcome {
                accepted: false,
                reading: self
                    .reading(now)
                    .expect("a duplicate can only exist after a window was created"),
                archived_reading: None,
            };
        }
        self.seen.insert(snapshot.id.clone());

        let Some(window) = self.window.as_mut() else {
            self.window = Some(new_window(snapshot, &self.config));
            return ApplyOutcome {
                accepted: true,
                reading: self
                    .reading(now)
                    .expect("the first snapshot created a window"),
                archived_reading: None,
            };
        };

        if snapshot.observed_at >= window.ends_at {
            let archived = reading_for(window, &self.config, now, None);
            self.window = Some(new_window(snapshot, &self.config));
            return ApplyOutcome {
                accepted: true,
                reading: self.reading(now).expect("expiry created the next window"),
                archived_reading: Some(archived),
            };
        }

        let previous_order = (
            &window.latest_snapshot.observed_at,
            &window.latest_snapshot.id,
        );
        let current_order = (&snapshot.observed_at, &snapshot.id);
        if current_order <= previous_order {
            return ApplyOutcome {
                accepted: false,
                reading: reading_for(window, &self.config, now, None),
                archived_reading: None,
            };
        }

        let reset_timestamp_changed = matches!(
            (window.latest_snapshot.resets_at, snapshot.resets_at),
            (Some(previous), Some(current)) if previous != current
        );
        let usage_decreased = snapshot.used_percent < window.latest_snapshot.used_percent;
        let delta = if reset_timestamp_changed || usage_decreased {
            snapshot.used_percent
        } else {
            snapshot.used_percent - window.latest_snapshot.used_percent
        };

        window.accumulated_weekly_points = (window.accumulated_weekly_points + delta).max(0.0);
        window.latest_snapshot = snapshot;

        let estimate = estimate_percent(
            window.accumulated_weekly_points,
            window.calibration_weekly_points,
        );
        let crossed_milestone =
            newest_unemitted_milestone(estimate, window.last_emitted_milestone, &self.config);
        if let Some(milestone) = crossed_milestone {
            window.last_emitted_milestone = Some(milestone);
        }

        ApplyOutcome {
            accepted: true,
            reading: reading_for(window, &self.config, now, crossed_milestone),
            archived_reading: None,
        }
    }

    pub fn apply_ordered<I>(&mut self, snapshots: I) -> Vec<ApplyOutcome>
    where
        I: IntoIterator<Item = WeeklySnapshot>,
    {
        let mut snapshots: Vec<_> = snapshots.into_iter().collect();
        snapshots.sort_by(|left, right| {
            (&left.observed_at, &left.id).cmp(&(&right.observed_at, &right.id))
        });
        snapshots
            .into_iter()
            .map(|snapshot| {
                let now = snapshot.observed_at;
                self.apply(snapshot, now)
            })
            .collect()
    }
}

fn new_window(snapshot: WeeklySnapshot, config: &TrackerConfig) -> LocalWindow {
    LocalWindow {
        started_at: snapshot.observed_at,
        ends_at: snapshot.observed_at + config.local_window_duration(),
        calibration_weekly_points: config.calibration_weekly_points(),
        latest_snapshot: snapshot,
        accumulated_weekly_points: 0.0,
        last_emitted_milestone: None,
    }
}

fn reading_for(
    window: &LocalWindow,
    config: &TrackerConfig,
    now: DateTime<Utc>,
    crossed_milestone: Option<u32>,
) -> MeterReading {
    let status = if now >= window.ends_at {
        WindowStatus::Expired
    } else if now - window.latest_snapshot.observed_at > config.stale_after() {
        WindowStatus::Stale
    } else {
        WindowStatus::Fresh
    };

    MeterReading {
        window_started_at: window.started_at,
        window_ends_at: window.ends_at,
        weekly_points: normalize_zero(window.accumulated_weekly_points),
        five_hour_estimate_percent: estimate_percent(
            window.accumulated_weekly_points,
            window.calibration_weekly_points,
        ),
        status,
        observed_at: window.latest_snapshot.observed_at,
        crossed_milestone,
    }
}

fn estimate_percent(weekly_points: f64, calibration: f64) -> f64 {
    normalize_zero((weekly_points.max(0.0) / calibration) * 100.0)
}

fn newest_unemitted_milestone(
    estimate: f64,
    last_emitted: Option<u32>,
    config: &TrackerConfig,
) -> Option<u32> {
    let last = last_emitted.unwrap_or(0);
    let mut candidate = config
        .warning_thresholds()
        .iter()
        .copied()
        .filter(|threshold| f64::from(*threshold) <= estimate && *threshold > last)
        .max();

    if estimate >= 100.0 {
        let step = config.super_usage_step();
        let super_candidate = 100 + (((estimate.floor() as u32).saturating_sub(100)) / step) * step;
        if super_candidate > 100 && super_candidate > last {
            candidate =
                Some(candidate.map_or(super_candidate, |current| current.max(super_candidate)));
        }
    }

    candidate
}

fn normalize_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

pub fn round_five_hour_percent(value: f64) -> i64 {
    normalize_zero(value.max(0.0)).round() as i64
}

pub fn round_weekly_points(value: f64) -> f64 {
    normalize_zero((value.max(0.0) * 10.0).round() / 10.0)
}
