use chrono::{DateTime, TimeDelta, Utc};
use codex_usage_watch::{
    AccountingEngine, DomainError, MeterReading, ObservationId, TrackerConfig, WeeklySnapshot,
    WindowStatus, round_five_hour_percent, round_weekly_points,
};

fn dt(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn snapshot(source: &str, offset: u64, at: &str, used: f64, reset: i64) -> WeeklySnapshot {
    WeeklySnapshot::new(
        ObservationId::new(source, offset),
        dt(at),
        used,
        DateTime::from_timestamp(reset, 0),
        10_080,
        Some("plus".to_owned()),
    )
    .unwrap()
}

fn expected_reading(
    start: &str,
    observed: &str,
    weekly_points: f64,
    status: WindowStatus,
    milestone: Option<u32>,
) -> MeterReading {
    MeterReading {
        window_started_at: dt(start),
        window_ends_at: dt(start) + TimeDelta::hours(5),
        weekly_points,
        five_hour_estimate_percent: weekly_points / 15.8 * 100.0,
        status,
        observed_at: dt(observed),
        crossed_milestone: milestone,
    }
}

#[test]
fn normal_growth_matches_the_hand_calculated_oracle() {
    let mut engine = AccountingEngine::new(TrackerConfig::default());
    let first = engine.apply(
        snapshot("normal", 0, "2030-01-01T12:00:00Z", 20.0, 1_893_974_400),
        dt("2030-01-01T12:00:00Z"),
    );
    assert_eq!(
        first.reading,
        expected_reading(
            "2030-01-01T12:00:00Z",
            "2030-01-01T12:00:00Z",
            0.0,
            WindowStatus::Fresh,
            None,
        )
    );

    let second = engine.apply(
        snapshot("normal", 256, "2030-01-01T12:05:00Z", 22.0, 1_893_974_400),
        dt("2030-01-01T12:05:00Z"),
    );
    assert_eq!(
        second,
        codex_usage_watch::ApplyOutcome {
            accepted: true,
            reading: expected_reading(
                "2030-01-01T12:00:00Z",
                "2030-01-01T12:05:00Z",
                2.0,
                WindowStatus::Fresh,
                None,
            ),
            archived_reading: None,
        }
    );
    assert_eq!(
        round_five_hour_percent(second.reading.five_hour_estimate_percent),
        13
    );
    assert_eq!(round_weekly_points(second.reading.weekly_points), 2.0);
}

#[test]
fn no_growth_stays_at_zero() {
    let snapshots = [
        snapshot("no-growth", 0, "2030-01-01T12:00:00Z", 20.0, 1_893_974_400),
        snapshot(
            "no-growth",
            256,
            "2030-01-01T12:05:00Z",
            20.0,
            1_893_974_400,
        ),
        snapshot(
            "no-growth",
            512,
            "2030-01-01T12:10:00Z",
            20.0,
            1_893_974_400,
        ),
    ];
    let mut engine = AccountingEngine::new(TrackerConfig::default());
    let outcomes = engine.apply_ordered(snapshots);
    assert_eq!(
        outcomes.last().unwrap().reading,
        expected_reading(
            "2030-01-01T12:00:00Z",
            "2030-01-01T12:10:00Z",
            0.0,
            WindowStatus::Fresh,
            None,
        )
    );
}

#[test]
fn expiry_archives_the_old_window_and_starts_a_zero_baseline() {
    let mut engine = AccountingEngine::new(TrackerConfig::default());
    engine.apply_ordered([
        snapshot("expiry", 0, "2030-01-01T12:00:00Z", 20.0, 1_893_974_400),
        snapshot("expiry", 256, "2030-01-01T16:59:59Z", 22.0, 1_893_974_400),
    ]);
    let outcome = engine.apply(
        snapshot("expiry", 512, "2030-01-01T17:00:00Z", 25.0, 1_893_974_400),
        dt("2030-01-01T17:00:00Z"),
    );

    assert_eq!(
        outcome.archived_reading,
        Some(expected_reading(
            "2030-01-01T12:00:00Z",
            "2030-01-01T16:59:59Z",
            2.0,
            WindowStatus::Expired,
            None,
        ))
    );
    assert_eq!(
        outcome.reading,
        expected_reading(
            "2030-01-01T17:00:00Z",
            "2030-01-01T17:00:00Z",
            0.0,
            WindowStatus::Fresh,
            None,
        )
    );
}

#[test]
fn weekly_reset_adds_only_confirmed_pre_and_post_reset_growth() {
    let mut engine = AccountingEngine::new(TrackerConfig::default());
    let outcomes = engine.apply_ordered([
        snapshot("reset", 0, "2030-01-01T12:00:00Z", 98.0, 1_893_542_400),
        snapshot("reset", 256, "2030-01-01T12:05:00Z", 99.0, 1_893_542_400),
        snapshot("reset", 512, "2030-01-01T12:10:00Z", 1.0, 1_894_147_200),
        snapshot("reset", 768, "2030-01-01T12:15:00Z", 3.0, 1_894_147_200),
    ]);

    let final_reading = &outcomes.last().unwrap().reading;
    assert_eq!(
        *final_reading,
        expected_reading(
            "2030-01-01T12:00:00Z",
            "2030-01-01T12:15:00Z",
            4.0,
            WindowStatus::Fresh,
            None,
        )
    );
    assert_eq!(
        round_five_hour_percent(final_reading.five_hour_estimate_percent),
        25
    );
}

#[test]
fn a_changed_reset_timestamp_is_reset_evidence_even_without_a_decrease() {
    let mut engine = AccountingEngine::new(TrackerConfig::default());
    engine.apply(
        snapshot("reset-id", 0, "2030-01-01T12:00:00Z", 5.0, 100),
        dt("2030-01-01T12:00:00Z"),
    );
    let result = engine.apply(
        snapshot("reset-id", 1, "2030-01-01T12:01:00Z", 5.0, 200),
        dt("2030-01-01T12:01:00Z"),
    );
    assert_eq!(result.reading.weekly_points, 5.0);
}

#[test]
fn duplicate_and_older_events_do_not_change_state() {
    let first = snapshot("session-a", 0, "2030-01-01T12:00:00Z", 20.0, 1_893_974_400);
    let second = snapshot(
        "session-a",
        256,
        "2030-01-01T12:05:00Z",
        21.0,
        1_893_974_400,
    );
    let older = snapshot("session-b", 0, "2030-01-01T12:02:00Z", 99.0, 1_893_974_400);
    let mut engine = AccountingEngine::new(TrackerConfig::default());
    engine.apply(first, dt("2030-01-01T12:00:00Z"));
    let accepted = engine.apply(second.clone(), dt("2030-01-01T12:05:00Z"));
    let duplicate = engine.apply(second, dt("2030-01-01T12:05:00Z"));
    let late_older = engine.apply(older, dt("2030-01-01T12:05:00Z"));

    assert!(accepted.accepted);
    assert!(!duplicate.accepted);
    assert!(!late_older.accepted);
    assert_eq!(duplicate.reading.weekly_points, 1.0);
    assert_eq!(late_older.reading.weekly_points, 1.0);
}

#[test]
fn concurrent_sessions_are_sorted_and_accumulated_once() {
    let mut engine = AccountingEngine::new(TrackerConfig::default());
    let outcomes = engine.apply_ordered([
        snapshot("a", 256, "2030-01-01T12:02:00Z", 21.0, 1_893_974_400),
        snapshot("b", 0, "2030-01-01T12:01:00Z", 21.0, 1_893_974_400),
        snapshot("a", 0, "2030-01-01T12:00:00Z", 20.0, 1_893_974_400),
        snapshot("b", 256, "2030-01-01T12:03:00Z", 22.0, 1_893_974_400),
    ]);
    assert_eq!(outcomes.last().unwrap().reading.weekly_points, 2.0);
}

#[test]
fn freshness_boundary_is_inclusive_and_expiry_has_priority() {
    let mut engine = AccountingEngine::new(TrackerConfig::default());
    engine.apply(
        snapshot("fresh", 0, "2030-01-01T12:00:00Z", 20.0, 1_893_974_400),
        dt("2030-01-01T12:00:00Z"),
    );

    assert_eq!(
        engine.reading(dt("2030-01-01T12:15:00Z")).unwrap().status,
        WindowStatus::Fresh
    );
    assert_eq!(
        engine.reading(dt("2030-01-01T12:15:01Z")).unwrap().status,
        WindowStatus::Stale
    );
    assert_eq!(
        engine.reading(dt("2030-01-01T17:00:00Z")).unwrap().status,
        WindowStatus::Expired
    );
}

#[test]
fn warnings_cross_base_and_super_usage_milestones_without_repeating() {
    let mut engine = AccountingEngine::new(TrackerConfig::default());
    engine.apply(
        snapshot("warnings", 0, "2030-01-01T12:00:00Z", 0.0, 1_893_974_400),
        dt("2030-01-01T12:00:00Z"),
    );
    let seventy_five = engine.apply(
        snapshot("warnings", 1, "2030-01-01T12:01:00Z", 12.0, 1_893_974_400),
        dt("2030-01-01T12:01:00Z"),
    );
    let one_ten = engine.apply(
        snapshot("warnings", 2, "2030-01-01T12:02:00Z", 18.0, 1_893_974_400),
        dt("2030-01-01T12:02:00Z"),
    );
    let unchanged = engine.apply(
        snapshot("warnings", 3, "2030-01-01T12:03:00Z", 18.0, 1_893_974_400),
        dt("2030-01-01T12:03:00Z"),
    );
    let one_twenty = engine.apply(
        snapshot("warnings", 4, "2030-01-01T12:04:00Z", 19.0, 1_893_974_400),
        dt("2030-01-01T12:04:00Z"),
    );

    assert_eq!(seventy_five.reading.crossed_milestone, Some(75));
    assert_eq!(one_ten.reading.crossed_milestone, Some(110));
    assert_eq!(unchanged.reading.crossed_milestone, None);
    assert_eq!(one_twenty.reading.crossed_milestone, Some(120));
    assert_eq!(
        round_five_hour_percent(one_ten.reading.five_hour_estimate_percent),
        114
    );
}

#[test]
fn invalid_domain_values_are_rejected_and_rounding_never_returns_negative_zero() {
    assert_eq!(
        WeeklySnapshot::new(
            ObservationId::new("bad", 0),
            dt("2030-01-01T12:00:00Z"),
            -1.0,
            None,
            10_080,
            None,
        ),
        Err(DomainError::InvalidWeeklyUsage)
    );
    assert_eq!(
        WeeklySnapshot::new(
            ObservationId::new("bad", 0),
            dt("2030-01-01T12:00:00Z"),
            1.0,
            None,
            300,
            None,
        ),
        Err(DomainError::NotWeeklyWindow)
    );
    assert_eq!(round_weekly_points(-0.01).to_bits(), 0.0_f64.to_bits());
    assert_eq!(round_five_hour_percent(113.924), 114);
}
