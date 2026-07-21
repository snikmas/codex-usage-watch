use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{
    FIVE_HOUR_WINDOW_MINUTES, ObservedRateLimitWindow, UsageObservation, WEEKLY_WINDOW_MINUTES,
};

/// Server reset timestamps observed in concurrent Codex sessions can differ by
/// a second. Treat shifts inside this single documented tolerance as jitter,
/// not as distinct quota epochs.
pub const RESET_TIMESTAMP_JITTER: TimeDelta = TimeDelta::seconds(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetClassification {
    NoReset,
    NaturalFiveHour,
    NaturalWeekly,
    InferredFull,
    Ambiguous,
    OldEpoch,
}

impl ResetClassification {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoReset => "no_reset",
            Self::NaturalFiveHour => "natural_five_hour",
            Self::NaturalWeekly => "natural_weekly",
            Self::InferredFull => "inferred_full",
            Self::Ambiguous => "ambiguous",
            Self::OldEpoch => "old_epoch",
        }
    }

    pub fn history_label(self) -> &'static str {
        match self {
            Self::NoReset => "no reset",
            Self::NaturalFiveHour => "natural 5h reset",
            Self::NaturalWeekly => "natural weekly reset",
            Self::InferredFull => "inferred full reset",
            Self::Ambiguous => "ambiguous reset",
            Self::OldEpoch => "old epoch ignored",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetDecision {
    pub classification: ResetClassification,
    pub boundary_at: Option<DateTime<Utc>>,
    pub reason: &'static str,
}

pub fn same_epoch(left: Option<DateTime<Utc>>, right: Option<DateTime<Utc>>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            right.signed_duration_since(left).abs() <= RESET_TIMESTAMP_JITTER
        }
        (None, None) => true,
        _ => false,
    }
}

pub fn inferred_epoch_start(window: &ObservedRateLimitWindow) -> Option<DateTime<Utc>> {
    let reset = window.resets_at?;
    let duration = TimeDelta::try_minutes(i64::from(window.window_minutes))?;
    Some(reset - duration)
}

pub fn classify_reset(previous: &UsageObservation, current: &UsageObservation) -> ResetDecision {
    debug_assert!(
        (previous.observed_at, &previous.id) <= (current.observed_at, &current.id),
        "reset observations must be ordered"
    );

    let five = compare_window(
        previous.five_hour.as_ref(),
        current.five_hour.as_ref(),
        FIVE_HOUR_WINDOW_MINUTES,
    );
    let weekly = compare_window(
        previous.weekly.as_ref(),
        current.weekly.as_ref(),
        WEEKLY_WINDOW_MINUTES,
    );

    if five == EpochChange::Old || weekly == EpochChange::Old {
        return decision(
            ResetClassification::OldEpoch,
            None,
            "a reset timestamp moved backwards beyond the jitter tolerance",
        );
    }
    if five == EpochChange::Invalid || weekly == EpochChange::Invalid {
        return decision(
            ResetClassification::Ambiguous,
            None,
            "missing or conflicting duration/reset evidence prevents classification",
        );
    }
    if five != EpochChange::Changed && weekly != EpochChange::Changed {
        return decision(
            ResetClassification::NoReset,
            None,
            "both quota epochs are unchanged within timestamp tolerance",
        );
    }

    let five_start = changed_start(
        previous.five_hour.as_ref(),
        current.five_hour.as_ref(),
        five,
    );
    let weekly_start = changed_start(previous.weekly.as_ref(), current.weekly.as_ref(), weekly);
    let five_natural =
        change_is_natural(previous.five_hour.as_ref(), five_start, current.observed_at);
    let weekly_natural =
        change_is_natural(previous.weekly.as_ref(), weekly_start, current.observed_at);

    if five == EpochChange::Changed && weekly == EpochChange::Changed {
        if !five_natural
            && !weekly_natural
            && five_start.zip(weekly_start).is_some_and(|(left, right)| {
                right.signed_duration_since(left).abs() <= RESET_TIMESTAMP_JITTER
            })
        {
            return decision(
                ResetClassification::InferredFull,
                clamp_boundary(five_start, previous.observed_at, current.observed_at),
                "both quota epochs restarted early with agreeing inferred starts; this is not proof that /usage was selected",
            );
        }
        if five_natural {
            return decision(
                ResetClassification::NaturalFiveHour,
                clamp_boundary(five_start, previous.observed_at, current.observed_at),
                "the previous five-hour deadline was reached before the new epoch began",
            );
        }
        return decision(
            ResetClassification::Ambiguous,
            clamp_boundary(
                five_start.or(weekly_start),
                previous.observed_at,
                current.observed_at,
            ),
            "both epochs changed but their timing does not prove one coherent early reset",
        );
    }

    if five == EpochChange::Changed {
        return if five_natural {
            decision(
                ResetClassification::NaturalFiveHour,
                clamp_boundary(five_start, previous.observed_at, current.observed_at),
                "the server five-hour epoch advanced after its advertised deadline",
            )
        } else {
            decision(
                ResetClassification::Ambiguous,
                clamp_boundary(five_start, previous.observed_at, current.observed_at),
                "the five-hour epoch changed early without matching weekly reset evidence",
            )
        };
    }

    if weekly_natural {
        decision(
            ResetClassification::NaturalWeekly,
            clamp_boundary(weekly_start, previous.observed_at, current.observed_at),
            "the weekly epoch advanced after its advertised deadline while the five-hour epoch continued",
        )
    } else {
        decision(
            ResetClassification::Ambiguous,
            clamp_boundary(weekly_start, previous.observed_at, current.observed_at),
            "the weekly epoch changed early without matching five-hour reset evidence",
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EpochChange {
    Missing,
    Same,
    Changed,
    Old,
    Invalid,
}

fn compare_window(
    previous: Option<&ObservedRateLimitWindow>,
    current: Option<&ObservedRateLimitWindow>,
    expected_minutes: u32,
) -> EpochChange {
    let (Some(previous), Some(current)) = (previous, current) else {
        return EpochChange::Missing;
    };
    if previous.window_minutes != expected_minutes || current.window_minutes != expected_minutes {
        return EpochChange::Invalid;
    }
    let (Some(previous_reset), Some(current_reset)) = (previous.resets_at, current.resets_at)
    else {
        return EpochChange::Invalid;
    };
    let shift = current_reset.signed_duration_since(previous_reset);
    if shift < -RESET_TIMESTAMP_JITTER {
        EpochChange::Old
    } else if shift.abs() <= RESET_TIMESTAMP_JITTER {
        EpochChange::Same
    } else {
        EpochChange::Changed
    }
}

fn changed_start(
    _previous: Option<&ObservedRateLimitWindow>,
    current: Option<&ObservedRateLimitWindow>,
    change: EpochChange,
) -> Option<DateTime<Utc>> {
    (change == EpochChange::Changed)
        .then(|| current.and_then(inferred_epoch_start))
        .flatten()
}

fn change_is_natural(
    previous: Option<&ObservedRateLimitWindow>,
    inferred_start: Option<DateTime<Utc>>,
    observed_at: DateTime<Utc>,
) -> bool {
    let Some(old_deadline) = previous.and_then(|window| window.resets_at) else {
        return false;
    };
    let evidence_time = inferred_start.unwrap_or(observed_at);
    evidence_time + RESET_TIMESTAMP_JITTER >= old_deadline
}

fn clamp_boundary(
    boundary: Option<DateTime<Utc>>,
    previous: DateTime<Utc>,
    current: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    boundary.map(|value| value.max(previous).min(current))
}

fn decision(
    classification: ResetClassification,
    boundary_at: Option<DateTime<Utc>>,
    reason: &'static str,
) -> ResetDecision {
    ResetDecision {
        classification,
        boundary_at,
        reason,
    }
}
