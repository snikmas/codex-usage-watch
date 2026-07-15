use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::ObservedRateLimitWindow;

pub const MINIMUM_RECOMMENDATION_SAMPLES: usize = 5;
pub const VALIDATED_MINIMUM_SAMPLES: usize = 10;
pub const IGNORED_MOVEMENT_MAX: f64 = 25.0;
pub const LOW_QUALITY_MOVEMENT_MAX: f64 = 50.0;
pub const USEFUL_MOVEMENT_MAX: f64 = 80.0;
pub const REPLACEMENT_DRIFT_PERCENT: f64 = 10.0;
pub const MAX_CANDIDATE_RELATIVE_IQR: f64 = 0.25;
pub const MAX_VALIDATED_RELATIVE_IQR: f64 = 0.10;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CalibrationIdentity {
    pub plan_type: String,
    pub model_slug: String,
    pub service_tier: String,
    pub schema_fingerprint: String,
    pub compatibility_generation: String,
}

impl CalibrationIdentity {
    pub fn key(&self) -> String {
        [
            self.plan_type.as_str(),
            self.model_slug.as_str(),
            self.service_tier.as_str(),
            self.schema_fingerprint.as_str(),
            self.compatibility_generation.as_str(),
        ]
        .join("\u{1f}")
    }

    pub fn supports_plus_baseline(&self) -> bool {
        self.plan_type.eq_ignore_ascii_case("plus")
            && !self.schema_fingerprint.starts_with("unsupported:")
            && self.schema_fingerprint != "unavailable"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationConfidence {
    Baseline,
    PersonalPreliminary,
    PersonalCandidate,
    PersonalValidated,
    InheritedUnvalidated,
    Unsupported,
}

impl CalibrationConfidence {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::PersonalPreliminary => "personal_preliminary",
            Self::PersonalCandidate => "personal_candidate",
            Self::PersonalValidated => "personal_validated",
            Self::InheritedUnvalidated => "inherited_unvalidated",
            Self::Unsupported => "unsupported",
        }
    }

    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "baseline" => Self::Baseline,
            "personal_preliminary" => Self::PersonalPreliminary,
            "personal_candidate" => Self::PersonalCandidate,
            "personal_validated" => Self::PersonalValidated,
            "inherited_unvalidated" => Self::InheritedUnvalidated,
            _ => Self::Unsupported,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationProfile {
    pub calibration_id: String,
    pub identity: CalibrationIdentity,
    pub value: Option<f64>,
    pub confidence: CalibrationConfidence,
    pub source: String,
    pub evidence_period_start: Option<DateTime<Utc>>,
    pub evidence_period_end: Option<DateTime<Utc>>,
    pub approved_at: Option<DateTime<Utc>>,
}

impl CalibrationProfile {
    pub fn plus_baseline(identity: CalibrationIdentity) -> Self {
        Self {
            calibration_id: stable_calibration_id("plus-historical-baseline-v1", &identity, 15.8),
            identity,
            value: Some(15.8),
            confidence: CalibrationConfidence::Baseline,
            source: "Plus historical paired-window dataset".to_string(),
            evidence_period_start: None,
            evidence_period_end: None,
            approved_at: None,
        }
    }

    pub fn unsupported(identity: CalibrationIdentity) -> Self {
        Self {
            calibration_id: stable_calibration_id("unsupported", &identity, 0.0),
            identity,
            value: None,
            confidence: CalibrationConfidence::Unsupported,
            source: "no exact-plan supported calibration".to_string(),
            evidence_period_start: None,
            evidence_period_end: None,
            approved_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CalibrationObservation {
    pub source_key: String,
    pub observed_at: DateTime<Utc>,
    pub five_hour: Option<ObservedRateLimitWindow>,
    pub weekly: Option<ObservedRateLimitWindow>,
    pub identity: CalibrationIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceQuality {
    Ignored,
    Low,
    Useful,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationSample {
    pub sample_key: String,
    pub identity: CalibrationIdentity,
    pub window_started_at: DateTime<Utc>,
    pub window_ended_at: DateTime<Utc>,
    pub five_hour_reset_at: DateTime<Utc>,
    pub weekly_reset_at: DateTime<Utc>,
    pub five_hour_delta_percent: f64,
    pub weekly_delta_points: f64,
    pub implied_weekly_points: f64,
    pub predicted_five_hour_percent: Option<f64>,
    pub prediction_error_percent: Option<f64>,
    pub quality: EvidenceQuality,
    pub eligible_for_estimator: bool,
    pub diagnostic_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundTruthStatus {
    Measured,
    NoNewGroundTruth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationReport {
    pub analyzed_at: DateTime<Utc>,
    pub calibration_id: String,
    pub identity: CalibrationIdentity,
    pub current_calibration: Option<f64>,
    pub confidence: CalibrationConfidence,
    pub confidence_reason: String,
    pub proposed_calibration: Option<f64>,
    pub recommend_change: bool,
    pub recommendation_reason: String,
    pub sample_count: usize,
    pub total_completed_window_count: usize,
    pub minimum_sample_count: usize,
    pub ignored_count: usize,
    pub low_quality_count: usize,
    pub useful_count: usize,
    pub high_quality_count: usize,
    pub weekly_only_observation_count: usize,
    pub excluded_group_count: usize,
    pub incompatible_identity_observation_count: usize,
    pub data_period_start: Option<DateTime<Utc>>,
    pub data_period_end: Option<DateTime<Utc>>,
    pub weighted_median: Option<f64>,
    pub median: Option<f64>,
    pub q1: Option<f64>,
    pub q3: Option<f64>,
    pub iqr: Option<f64>,
    pub relative_iqr: Option<f64>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub outlier_count: usize,
    pub last_ground_truth_at: Option<DateTime<Utc>>,
    pub detected_drift_percent: Option<f64>,
    pub drift_confirmation_count: usize,
    pub ground_truth_status: GroundTruthStatus,
    pub early_report_due: bool,
    pub weekly_report_due: bool,
    pub samples: Vec<CalibrationSample>,
}

pub(crate) fn build_report(
    mut observations: Vec<CalibrationObservation>,
    active_profile: CalibrationProfile,
    analyzed_at: DateTime<Utc>,
    drift_confirmation_count: usize,
) -> CalibrationReport {
    observations.sort_by_key(|item| item.observed_at);
    let target_identity = observations
        .last()
        .map(|item| item.identity.clone())
        .unwrap_or_else(|| active_profile.identity.clone());
    let incompatible_identity_observation_count = observations
        .iter()
        .filter(|item| item.identity != target_identity)
        .count();
    observations.retain(|item| item.identity == target_identity);
    let data_period_start = observations.first().map(|item| item.observed_at);
    let data_period_end = observations.last().map(|item| item.observed_at);
    let weekly_only_observation_count = observations
        .iter()
        .filter(|item| item.weekly.is_some() && item.five_hour.is_none())
        .count();

    let mut grouped: BTreeMap<DateTime<Utc>, Vec<CalibrationObservation>> = BTreeMap::new();
    let mut excluded_group_count = 0;
    for observation in observations {
        match observation
            .five_hour
            .as_ref()
            .and_then(|window| window.resets_at)
        {
            Some(reset) if observation.weekly.is_some() => {
                grouped.entry(reset).or_default().push(observation);
            }
            _ => excluded_group_count += 1,
        }
    }

    let mut samples = Vec::new();
    for (five_hour_reset_at, mut group) in grouped {
        group.sort_by_key(|item| item.observed_at);
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        if group.len() < 2 {
            excluded_group_count += 1;
            continue;
        }
        let (Some(first_five), Some(last_five), Some(first_weekly), Some(last_weekly)) = (
            first.five_hour.as_ref(),
            last.five_hour.as_ref(),
            first.weekly.as_ref(),
            last.weekly.as_ref(),
        ) else {
            excluded_group_count += 1;
            continue;
        };
        let compatible_resets = group.iter().all(|item| {
            item.five_hour.as_ref().and_then(|value| value.resets_at) == Some(five_hour_reset_at)
                && item.weekly.as_ref().and_then(|value| value.resets_at) == first_weekly.resets_at
        });
        let Some(weekly_reset_at) = first_weekly.resets_at else {
            excluded_group_count += 1;
            continue;
        };
        if !compatible_resets {
            excluded_group_count += 1;
            continue;
        }
        let five_delta = last_five.used_percent - first_five.used_percent;
        let weekly_delta = last_weekly.used_percent - first_weekly.used_percent;
        if five_delta <= 0.0
            || weekly_delta < 0.0
            || !five_delta.is_finite()
            || !weekly_delta.is_finite()
        {
            excluded_group_count += 1;
            continue;
        }
        let implied = weekly_delta * 100.0 / five_delta;
        if !implied.is_finite() || implied <= 0.0 {
            excluded_group_count += 1;
            continue;
        }
        let (quality, eligible, reason) = classify_movement(five_delta);
        let predicted = active_profile
            .value
            .map(|value| weekly_delta / value * 100.0);
        samples.push(CalibrationSample {
            sample_key: format!(
                "{}|{}|{}|{}|{}",
                target_identity.key(),
                five_hour_reset_at.to_rfc3339(),
                first.source_key,
                first.observed_at.to_rfc3339(),
                last.observed_at.to_rfc3339()
            ),
            identity: target_identity.clone(),
            window_started_at: first.observed_at,
            window_ended_at: last.observed_at,
            five_hour_reset_at,
            weekly_reset_at,
            five_hour_delta_percent: five_delta,
            weekly_delta_points: weekly_delta,
            implied_weekly_points: implied,
            predicted_five_hour_percent: predicted,
            prediction_error_percent: predicted.map(|value| value - five_delta),
            quality,
            eligible_for_estimator: eligible,
            diagnostic_reason: reason.to_string(),
        });
    }

    samples.sort_by_key(|item| item.window_started_at);
    let eligible: Vec<&CalibrationSample> = samples
        .iter()
        .filter(|item| item.eligible_for_estimator)
        .collect();
    let mut values: Vec<f64> = eligible
        .iter()
        .map(|item| item.implied_weekly_points)
        .collect();
    values.sort_by(f64::total_cmp);
    let weighted_median = weighted_median(&eligible);
    let median = percentile(&values, 0.5);
    let q1 = percentile(&values, 0.25);
    let q3 = percentile(&values, 0.75);
    let iqr = q1.zip(q3).map(|(low, high)| high - low);
    let relative_iqr = iqr
        .zip(weighted_median)
        .and_then(|(spread, center)| (center > 0.0).then_some(spread / center));
    let outlier_count = q1
        .zip(q3)
        .map(|(low, high)| {
            let spread = high - low;
            values
                .iter()
                .filter(|value| **value < low - 1.5 * spread || **value > high + 1.5 * spread)
                .count()
        })
        .unwrap_or(0);
    let useful_count = samples
        .iter()
        .filter(|item| item.quality == EvidenceQuality::Useful)
        .count();
    let high_quality_count = samples
        .iter()
        .filter(|item| item.quality == EvidenceQuality::High)
        .count();
    let sample_count = useful_count + high_quality_count;
    let proposed_calibration = (sample_count >= MINIMUM_RECOMMENDATION_SAMPLES)
        .then_some(weighted_median)
        .flatten();
    let detected_drift_percent = active_profile
        .value
        .zip(weighted_median)
        .map(|(current, value)| (value - current) / current * 100.0);
    let low_spread = relative_iqr.is_some_and(|value| value <= MAX_CANDIDATE_RELATIVE_IQR);
    let enough_drift =
        detected_drift_percent.is_some_and(|value| value.abs() >= REPLACEMENT_DRIFT_PERCENT);
    let recommend_change = proposed_calibration.is_some()
        && enough_drift
        && low_spread
        && drift_confirmation_count >= 2;
    let confidence = evidence_confidence(
        &active_profile,
        sample_count,
        high_quality_count,
        relative_iqr,
        drift_confirmation_count,
    );
    let confidence_reason =
        confidence_reason(confidence, sample_count, high_quality_count, relative_iqr);
    let recommendation_reason = if sample_count < MINIMUM_RECOMMENDATION_SAMPLES {
        format!(
            "{sample_count} useful/high-quality compatible windows; at least {MINIMUM_RECOMMENDATION_SAMPLES} are required"
        )
    } else if !enough_drift {
        format!("difference is below the named {REPLACEMENT_DRIFT_PERCENT:.0}% drift threshold")
    } else if !low_spread {
        "candidate spread is too high for replacement".to_string()
    } else if drift_confirmation_count < 2 {
        "candidate requires confirmation with new compatible evidence in a later analysis"
            .to_string()
    } else {
        "persistent, sufficiently large, low-spread drift; review before applying".to_string()
    };
    let weekly_report_due = data_period_start
        .zip(data_period_end)
        .is_some_and(|(start, end)| end - start >= chrono::TimeDelta::days(7));
    let ground_truth_status = if samples.is_empty() {
        GroundTruthStatus::NoNewGroundTruth
    } else {
        GroundTruthStatus::Measured
    };

    CalibrationReport {
        analyzed_at,
        calibration_id: active_profile.calibration_id,
        identity: target_identity,
        current_calibration: active_profile.value,
        confidence,
        confidence_reason,
        proposed_calibration,
        recommend_change,
        recommendation_reason,
        sample_count,
        total_completed_window_count: samples.len(),
        minimum_sample_count: MINIMUM_RECOMMENDATION_SAMPLES,
        ignored_count: samples
            .iter()
            .filter(|item| item.quality == EvidenceQuality::Ignored)
            .count(),
        low_quality_count: samples
            .iter()
            .filter(|item| item.quality == EvidenceQuality::Low)
            .count(),
        useful_count,
        high_quality_count,
        weekly_only_observation_count,
        excluded_group_count,
        incompatible_identity_observation_count,
        data_period_start,
        data_period_end,
        weighted_median,
        median,
        q1,
        q3,
        iqr,
        relative_iqr,
        minimum: values.first().copied(),
        maximum: values.last().copied(),
        outlier_count,
        last_ground_truth_at: samples.iter().map(|item| item.window_ended_at).max(),
        detected_drift_percent,
        drift_confirmation_count,
        ground_truth_status,
        early_report_due: sample_count >= MINIMUM_RECOMMENDATION_SAMPLES,
        weekly_report_due,
        samples,
    }
}

fn classify_movement(movement: f64) -> (EvidenceQuality, bool, &'static str) {
    if movement < IGNORED_MOVEMENT_MAX {
        (
            EvidenceQuality::Ignored,
            false,
            "five-hour movement below 25 points; rounded movement ignored",
        )
    } else if movement < LOW_QUALITY_MOVEMENT_MAX {
        (
            EvidenceQuality::Low,
            false,
            "25-49 point movement retained as low quality but excluded from estimator",
        )
    } else if movement < USEFUL_MOVEMENT_MAX {
        (
            EvidenceQuality::Useful,
            true,
            "50-79 point movement is useful calibration evidence",
        )
    } else {
        (
            EvidenceQuality::High,
            true,
            "80-100 point movement is high-quality calibration evidence",
        )
    }
}

fn evidence_confidence(
    profile: &CalibrationProfile,
    sample_count: usize,
    high_count: usize,
    relative_iqr: Option<f64>,
    confirmation_count: usize,
) -> CalibrationConfidence {
    if profile.confidence == CalibrationConfidence::Unsupported {
        return CalibrationConfidence::Unsupported;
    }
    if sample_count >= VALIDATED_MINIMUM_SAMPLES
        && high_count >= 5
        && relative_iqr.is_some_and(|value| value <= MAX_VALIDATED_RELATIVE_IQR)
        && confirmation_count >= 2
    {
        CalibrationConfidence::PersonalValidated
    } else if sample_count >= MINIMUM_RECOMMENDATION_SAMPLES {
        CalibrationConfidence::PersonalCandidate
    } else if sample_count >= 2 {
        CalibrationConfidence::PersonalPreliminary
    } else {
        profile.confidence
    }
}

fn confidence_reason(
    confidence: CalibrationConfidence,
    samples: usize,
    high: usize,
    relative_iqr: Option<f64>,
) -> String {
    format!(
        "{confidence:?}: {samples} useful/high samples, {high} high-quality, relative IQR {}",
        relative_iqr
            .map(|value| format!("{:.1}%", value * 100.0))
            .unwrap_or_else(|| "unavailable".to_string())
    )
}

fn weighted_median(samples: &[&CalibrationSample]) -> Option<f64> {
    let mut weighted: Vec<(f64, f64)> = samples
        .iter()
        .map(|sample| (sample.implied_weekly_points, sample.five_hour_delta_percent))
        .collect();
    weighted.sort_by(|left, right| left.0.total_cmp(&right.0));
    let total: f64 = weighted.iter().map(|item| item.1).sum();
    let mut cumulative = 0.0;
    for (value, weight) in weighted {
        cumulative += weight;
        if cumulative >= total / 2.0 {
            return Some(value);
        }
    }
    None
}

fn percentile(values: &[f64], fraction: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let position = fraction * (values.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let weight = position - lower as f64;
    Some(values[lower] * (1.0 - weight) + values[upper] * weight)
}

pub(crate) fn stable_calibration_id(
    source: &str,
    identity: &CalibrationIdentity,
    value: f64,
) -> String {
    let input = format!("{source}|{}|{value:.8}", identity.key());
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("cal-{hash:016x}")
}
