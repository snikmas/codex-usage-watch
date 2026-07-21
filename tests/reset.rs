use chrono::{DateTime, Utc};
use codex_usage_watch::{
    ObservationId, ObservedRateLimitWindow, ResetClassification, UsageObservation, classify_reset,
};

fn dt(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn observation(
    offset: u64,
    at: &str,
    five_used: f64,
    five_reset: Option<&str>,
    weekly_used: f64,
    weekly_reset: Option<&str>,
) -> UsageObservation {
    UsageObservation {
        id: ObservationId::new("reset.jsonl", offset),
        observed_at: dt(at),
        five_hour: Some(ObservedRateLimitWindow {
            used_percent: five_used,
            window_minutes: 300,
            resets_at: five_reset.map(dt),
        }),
        weekly: Some(ObservedRateLimitWindow {
            used_percent: weekly_used,
            window_minutes: 10_080,
            resets_at: weekly_reset.map(dt),
        }),
        model_slug: None,
        codex_version: None,
        service_tier: None,
        plan_type: Some("plus".to_string()),
        schema_fingerprint: "test".to_string(),
    }
}

#[test]
fn classifies_natural_five_hour_and_weekly_boundaries_by_duration() {
    let before_five = observation(
        0,
        "2030-01-01T12:00:00Z",
        99.0,
        Some("2030-01-01T13:00:00Z"),
        20.0,
        Some("2030-01-08T00:00:00Z"),
    );
    let after_five = observation(
        1,
        "2030-01-01T13:02:00Z",
        2.0,
        Some("2030-01-01T18:00:00Z"),
        21.0,
        Some("2030-01-08T00:00:00Z"),
    );
    let decision = classify_reset(&before_five, &after_five);
    assert_eq!(
        decision.classification,
        ResetClassification::NaturalFiveHour
    );
    assert_eq!(decision.boundary_at, Some(dt("2030-01-01T13:00:00Z")));

    let before_weekly = observation(
        2,
        "2030-01-01T12:00:00Z",
        10.0,
        Some("2030-01-01T15:00:00Z"),
        99.0,
        Some("2030-01-01T12:30:00Z"),
    );
    let after_weekly = observation(
        3,
        "2030-01-01T12:31:00Z",
        11.0,
        Some("2030-01-01T15:00:00Z"),
        2.0,
        Some("2030-01-08T12:30:00Z"),
    );
    assert_eq!(
        classify_reset(&before_weekly, &after_weekly).classification,
        ResetClassification::NaturalWeekly
    );
}

#[test]
fn simultaneous_early_epochs_infer_full_reset_without_requiring_a_decrease() {
    let before = observation(
        0,
        "2030-01-01T12:00:00Z",
        1.0,
        Some("2030-01-01T15:00:00Z"),
        1.0,
        Some("2030-01-08T00:00:00Z"),
    );
    let after = observation(
        1,
        "2030-01-01T12:10:00Z",
        2.0,
        Some("2030-01-01T17:10:00Z"),
        2.0,
        Some("2030-01-08T12:10:00Z"),
    );
    let decision = classify_reset(&before, &after);
    assert_eq!(decision.classification, ResetClassification::InferredFull);
    assert_eq!(decision.boundary_at, Some(dt("2030-01-01T12:10:00Z")));
    assert!(decision.reason.contains("not proof"));
}

#[test]
fn jitter_is_not_a_reset_and_backwards_epochs_are_rejected() {
    let before = observation(
        0,
        "2030-01-01T12:00:00Z",
        10.0,
        Some("2030-01-01T15:00:00Z"),
        20.0,
        Some("2030-01-08T00:00:00Z"),
    );
    let jitter = observation(
        1,
        "2030-01-01T12:01:00Z",
        11.0,
        Some("2030-01-01T15:00:01Z"),
        21.0,
        Some("2030-01-08T00:00:01Z"),
    );
    assert_eq!(
        classify_reset(&before, &jitter).classification,
        ResetClassification::NoReset
    );

    let old = observation(
        2,
        "2030-01-01T12:02:00Z",
        12.0,
        Some("2029-12-31T15:00:00Z"),
        22.0,
        Some("2030-01-01T00:00:00Z"),
    );
    assert_eq!(
        classify_reset(&jitter, &old).classification,
        ResetClassification::OldEpoch
    );
}

#[test]
fn missing_or_one_sided_early_evidence_is_ambiguous() {
    let before = observation(
        0,
        "2030-01-01T12:00:00Z",
        10.0,
        Some("2030-01-01T15:00:00Z"),
        20.0,
        Some("2030-01-08T00:00:00Z"),
    );
    let one_sided = observation(
        1,
        "2030-01-01T12:10:00Z",
        1.0,
        Some("2030-01-01T17:10:00Z"),
        21.0,
        Some("2030-01-08T00:00:00Z"),
    );
    assert_eq!(
        classify_reset(&before, &one_sided).classification,
        ResetClassification::Ambiguous
    );

    let missing = observation(
        2,
        "2030-01-01T12:20:00Z",
        2.0,
        None,
        22.0,
        Some("2030-01-08T00:00:00Z"),
    );
    assert_eq!(
        classify_reset(&one_sided, &missing).classification,
        ResetClassification::Ambiguous
    );
}
