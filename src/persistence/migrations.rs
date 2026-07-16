use chrono::Utc;
use rusqlite::{Connection, TransactionBehavior, params};

use super::{CalibrationKind, SCHEMA_VERSION, StateError};
use crate::model::TrackerConfig;

pub(super) fn migrate(connection: &mut Connection) -> Result<(), StateError> {
    migrate_to(connection, SCHEMA_VERSION)
}

fn migrate_to(connection: &mut Connection, target_version: i64) -> Result<(), StateError> {
    debug_assert!((1..=SCHEMA_VERSION).contains(&target_version));
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(StateError::UnsupportedSchemaVersion {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    if version < 1 && target_version >= 1 {
        transaction.execute_batch(
            "CREATE TABLE schema_migrations (
                 version INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL
             );
             CREATE TABLE config_metadata (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 calibration_weekly_points REAL NOT NULL CHECK (calibration_weekly_points > 0),
                 local_window_seconds INTEGER NOT NULL CHECK (local_window_seconds > 0),
                 stale_after_seconds INTEGER NOT NULL CHECK (stale_after_seconds > 0),
                 warning_thresholds_json TEXT NOT NULL,
                 super_usage_step INTEGER NOT NULL CHECK (super_usage_step > 0),
                 calibration_kind TEXT NOT NULL,
                 updated_at TEXT NOT NULL
             );
             CREATE TABLE snapshots (
                 id INTEGER PRIMARY KEY,
                 source_file TEXT NOT NULL,
                 byte_offset INTEGER NOT NULL CHECK (byte_offset >= 0),
                 observed_at TEXT NOT NULL,
                 used_percent REAL NOT NULL CHECK (used_percent >= 0 AND used_percent <= 100),
                 resets_at TEXT,
                 window_minutes INTEGER NOT NULL CHECK (window_minutes = 10080),
                 plan_type TEXT,
                 affects_meter INTEGER NOT NULL DEFAULT 0 CHECK (affects_meter IN (0, 1)),
                 UNIQUE (source_file, byte_offset)
             );
             CREATE INDEX snapshots_order ON snapshots(observed_at, source_file, byte_offset);
             CREATE TABLE windows (
                 started_at TEXT PRIMARY KEY,
                 ends_at TEXT NOT NULL,
                 latest_observed_at TEXT NOT NULL,
                 latest_used_percent REAL NOT NULL,
                 calibration_weekly_points REAL NOT NULL CHECK (calibration_weekly_points > 0),
                 accumulated_weekly_points REAL NOT NULL CHECK (accumulated_weekly_points >= 0),
                 last_emitted_milestone INTEGER,
                 lifecycle TEXT NOT NULL CHECK (lifecycle IN ('current', 'archived'))
             );
             CREATE UNIQUE INDEX one_current_window ON windows(lifecycle) WHERE lifecycle = 'current';
             CREATE TABLE emitted_warnings (
                 window_started_at TEXT NOT NULL,
                 milestone INTEGER NOT NULL,
                 emitted_at TEXT NOT NULL,
                 PRIMARY KEY (window_started_at, milestone)
             );"
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![1, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", 1)?;
    }
    if version < 2 && target_version >= 2 {
        transaction.execute_batch(
            "CREATE TABLE rate_limit_observations (
                 source_file TEXT NOT NULL,
                 byte_offset INTEGER NOT NULL CHECK (byte_offset >= 0),
                 observed_at TEXT NOT NULL,
                 model_slug TEXT,
                 service_tier TEXT,
                 plan_type TEXT,
                 schema_fingerprint TEXT NOT NULL,
                 PRIMARY KEY (source_file, byte_offset)
             );
             CREATE INDEX observations_order
                 ON rate_limit_observations(observed_at, source_file, byte_offset);
             CREATE TABLE observed_rate_limit_windows (
                 source_file TEXT NOT NULL,
                 byte_offset INTEGER NOT NULL CHECK (byte_offset >= 0),
                 window_kind TEXT NOT NULL CHECK (window_kind IN ('five_hour', 'weekly')),
                 used_percent REAL NOT NULL CHECK (used_percent >= 0 AND used_percent <= 100),
                 window_minutes INTEGER NOT NULL CHECK (window_minutes IN (300, 10080)),
                 resets_at TEXT,
                 PRIMARY KEY (source_file, byte_offset, window_kind),
                 FOREIGN KEY (source_file, byte_offset)
                     REFERENCES rate_limit_observations(source_file, byte_offset)
                     ON DELETE CASCADE
             );
             CREATE TABLE diagnostic_events (
                 source_file TEXT NOT NULL,
                 byte_offset INTEGER NOT NULL CHECK (byte_offset >= 0),
                 observed_at TEXT,
                 code TEXT NOT NULL,
                 field TEXT NOT NULL DEFAULT '',
                 message TEXT NOT NULL,
                 PRIMARY KEY (source_file, byte_offset, code, field)
             );
             CREATE TABLE transcript_cursors (
                 source_file TEXT PRIMARY KEY,
                 next_offset INTEGER NOT NULL CHECK (next_offset >= 0),
                 updated_at TEXT NOT NULL
             );",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![2, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", 2)?;
    }
    if version < 3 && target_version >= 3 {
        transaction.execute_batch(
            "ALTER TABLE transcript_cursors
                 ADD COLUMN continuity_marker BLOB NOT NULL DEFAULT X'';
             ALTER TABLE transcript_cursors
                 ADD COLUMN generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0);",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![3, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", 3)?;
    }
    if version < 4 && target_version >= 4 {
        transaction.execute_batch(
            "CREATE TABLE calibration_samples (
                 sample_key TEXT PRIMARY KEY,
                 window_started_at TEXT NOT NULL,
                 window_ended_at TEXT NOT NULL,
                 five_hour_reset_at TEXT NOT NULL,
                 weekly_reset_at TEXT NOT NULL,
                 five_hour_delta_percent REAL NOT NULL CHECK (five_hour_delta_percent > 0),
                 weekly_delta_points REAL NOT NULL CHECK (weekly_delta_points >= 0),
                 implied_weekly_points REAL NOT NULL CHECK (implied_weekly_points > 0),
                 model_slug TEXT,
                 service_tier TEXT,
                 plan_type TEXT,
                 created_at TEXT NOT NULL
             );
             CREATE TABLE calibration_applications (
                 id INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL,
                 calibration_weekly_points REAL NOT NULL CHECK (calibration_weekly_points > 0),
                 sample_count INTEGER NOT NULL CHECK (sample_count >= 0)
             );",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![4, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", 4)?;
    }
    if version < 5 && target_version >= 5 {
        transaction.execute_batch(
            "CREATE TABLE compatibility_results (
                 identity_key TEXT PRIMARY KEY,
                 codex_version TEXT NOT NULL,
                 model_slug TEXT NOT NULL,
                 service_tier TEXT NOT NULL,
                 schema_fingerprint TEXT NOT NULL,
                 result TEXT NOT NULL CHECK (result IN ('compatible', 'review', 'degraded')),
                 model_confidence TEXT NOT NULL,
                 hook_check TEXT NOT NULL,
                 transcript_check TEXT NOT NULL,
                 rate_limit_check TEXT NOT NULL,
                 projection_check TEXT NOT NULL,
                 tracker_version TEXT NOT NULL,
                 plugin_version TEXT NOT NULL,
                 checked_at TEXT NOT NULL
             );",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![5, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", 5)?;
    }
    if version < 6 && target_version >= 6 {
        transaction.execute_batch(
            "ALTER TABLE calibration_samples
                 ADD COLUMN predicted_five_hour_percent REAL NOT NULL DEFAULT 0;
             ALTER TABLE calibration_samples
                 ADD COLUMN prediction_error_percent REAL NOT NULL DEFAULT 0;",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![6, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", 6)?;
    }
    if version < 7 && target_version >= 7 {
        transaction.execute_batch(
            "ALTER TABLE config_metadata
                 ADD COLUMN current_compatibility_generation TEXT NOT NULL DEFAULT 'unknown';
             ALTER TABLE rate_limit_observations
                 ADD COLUMN compatibility_generation TEXT NOT NULL DEFAULT 'unknown';
             ALTER TABLE windows
                 ADD COLUMN calibration_id TEXT NOT NULL DEFAULT 'legacy-historical-15.8';
             ALTER TABLE windows
                 ADD COLUMN calibration_confidence TEXT NOT NULL DEFAULT 'inherited_unvalidated';
             ALTER TABLE calibration_samples
                 ADD COLUMN identity_key TEXT NOT NULL DEFAULT 'legacy';
             ALTER TABLE calibration_samples
                 ADD COLUMN schema_fingerprint TEXT NOT NULL DEFAULT 'legacy';
             ALTER TABLE calibration_samples
                 ADD COLUMN compatibility_generation TEXT NOT NULL DEFAULT 'unknown';
             ALTER TABLE calibration_samples
                 ADD COLUMN quality TEXT NOT NULL DEFAULT 'ignored';
             ALTER TABLE calibration_samples
                 ADD COLUMN eligible_for_estimator INTEGER NOT NULL DEFAULT 0 CHECK (eligible_for_estimator IN (0, 1));
             ALTER TABLE calibration_samples
                 ADD COLUMN diagnostic_reason TEXT NOT NULL DEFAULT 'legacy sample; reanalysis required';
             ALTER TABLE calibration_applications
                 ADD COLUMN calibration_id TEXT NOT NULL DEFAULT 'legacy';
             ALTER TABLE calibration_applications
                 ADD COLUMN identity_key TEXT NOT NULL DEFAULT 'legacy';
             ALTER TABLE compatibility_results
                 ADD COLUMN plan_type TEXT NOT NULL DEFAULT 'unknown';
             CREATE TABLE calibration_profiles (
                 calibration_id TEXT PRIMARY KEY,
                 identity_key TEXT NOT NULL,
                 plan_type TEXT NOT NULL,
                 model_slug TEXT NOT NULL,
                 service_tier TEXT NOT NULL,
                 schema_fingerprint TEXT NOT NULL,
                 compatibility_generation TEXT NOT NULL,
                 value REAL NOT NULL CHECK (value > 0),
                 confidence TEXT NOT NULL CHECK (confidence IN (
                     'baseline', 'personal_preliminary', 'personal_candidate',
                     'personal_validated', 'inherited_unvalidated', 'unsupported'
                 )),
                 source TEXT NOT NULL,
                 evidence_period_start TEXT,
                 evidence_period_end TEXT,
                 approved_at TEXT NOT NULL
             );
             CREATE INDEX calibration_profiles_identity
                 ON calibration_profiles(identity_key, approved_at DESC);
             CREATE TABLE calibration_analysis_state (
                 identity_key TEXT PRIMARY KEY,
                 last_sample_count INTEGER NOT NULL CHECK (last_sample_count >= 0),
                 candidate_value REAL,
                 confirmation_count INTEGER NOT NULL CHECK (confirmation_count >= 0),
                 updated_at TEXT NOT NULL
             );",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![7, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", 7)?;
    }
    if version < 8 && target_version >= 8 {
        transaction.execute_batch(
            "CREATE TABLE calibration_reports (
                 report_key TEXT PRIMARY KEY,
                 identity_key TEXT NOT NULL,
                 generated_at TEXT NOT NULL,
                 reason TEXT NOT NULL CHECK (reason IN (
                     'initial', 'weekly', 'five_new_qualifying_windows'
                 )),
                 sample_count INTEGER NOT NULL CHECK (sample_count >= 0),
                 report_json TEXT NOT NULL
             );
             CREATE INDEX calibration_reports_identity
                 ON calibration_reports(identity_key, generated_at DESC);",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![8, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", 8)?;
    }
    if version < 9 && target_version >= 9 {
        transaction.execute_batch(
            "ALTER TABLE rate_limit_observations
                 ADD COLUMN codex_version TEXT;",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![9, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", 9)?;
    }
    if version < 10 && target_version >= 10 {
        transaction.execute_batch(
            "CREATE TABLE control_events (
                 id INTEGER PRIMARY KEY,
                 occurred_at TEXT NOT NULL,
                 event_type TEXT NOT NULL CHECK (event_type IN ('manual_reset')),
                 detail TEXT NOT NULL
             );
             CREATE INDEX control_events_time
                 ON control_events(occurred_at DESC);",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![10, Utc::now().to_rfc3339()],
        )?;
        transaction.pragma_update(None, "user_version", 10)?;
    }
    transaction.commit()?;
    Ok(())
}

pub(super) fn write_config_metadata(
    connection: &Connection,
    config: &TrackerConfig,
) -> Result<(), StateError> {
    connection.execute(
        "INSERT INTO config_metadata (
             singleton, calibration_weekly_points, local_window_seconds,
             stale_after_seconds, warning_thresholds_json, super_usage_step,
             calibration_kind, updated_at
         ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(singleton) DO UPDATE SET
             local_window_seconds = excluded.local_window_seconds,
             stale_after_seconds = excluded.stale_after_seconds,
             warning_thresholds_json = excluded.warning_thresholds_json,
             super_usage_step = excluded.super_usage_step,
             updated_at = excluded.updated_at",
        params![
            config.calibration_weekly_points(),
            config.local_window_duration().num_seconds(),
            config.stale_after().num_seconds(),
            serde_json::to_string(config.warning_thresholds())?,
            config.super_usage_step(),
            CalibrationKind::Historical.as_str(),
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone, Utc};
    use rusqlite::{Connection, params};

    use super::{SCHEMA_VERSION, migrate_to, write_config_metadata};
    use crate::calibration::CalibrationConfidence;
    use crate::model::{TrackerConfig, WindowStatus};
    use crate::persistence::StateStore;

    #[test]
    fn every_historical_schema_migrates_without_losing_supported_state() {
        for historical_version in 1..SCHEMA_VERSION {
            let directory = tempfile::tempdir().unwrap();
            let database = directory.path().join("state.sqlite3");
            let config = TrackerConfig::default();
            let expected_calibration_id = if historical_version >= 7 {
                format!("fixture-calibration-v{historical_version}")
            } else {
                "legacy-historical-15.8".to_string()
            };

            let mut connection = Connection::open(&database).unwrap();
            migrate_to(&mut connection, historical_version).unwrap();
            write_config_metadata(&connection, &config).unwrap();
            seed_historical_fixture(&connection, historical_version, &expected_calibration_id);
            drop(connection);

            let mut store = StateStore::open_in(directory.path(), config).unwrap();
            assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
            assert_eq!(
                store.snapshot_count().unwrap(),
                1,
                "schema v{historical_version}"
            );
            assert_eq!(
                store.observation_count().unwrap(),
                usize::from(historical_version >= 2),
                "schema v{historical_version}"
            );
            assert_eq!(
                store.diagnostic_count().unwrap(),
                usize::from(historical_version >= 2),
                "schema v{historical_version}"
            );

            let (calibration_id, confidence): (String, String) = store
                .connection
                .query_row(
                    "SELECT calibration_id, calibration_confidence FROM windows",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(calibration_id, expected_calibration_id);
            assert_eq!(
                CalibrationConfidence::parse(&confidence),
                if historical_version >= 7 {
                    CalibrationConfidence::Baseline
                } else {
                    CalibrationConfidence::InheritedUnvalidated
                }
            );

            let cursor_count: i64 = store
                .connection
                .query_row("SELECT COUNT(*) FROM transcript_cursors", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(cursor_count, i64::from(historical_version >= 2));
            if historical_version >= 2 {
                let (offset, marker, generation): (i64, Vec<u8>, i64) = store
                    .connection
                    .query_row(
                        "SELECT next_offset, continuity_marker, generation FROM transcript_cursors",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .unwrap();
                assert_eq!(offset, 128);
                assert_eq!(
                    generation,
                    if historical_version >= 3 {
                        historical_version
                    } else {
                        0
                    }
                );
                assert_eq!(
                    marker,
                    if historical_version >= 3 {
                        vec![0xCA, 0xFE]
                    } else {
                        Vec::new()
                    }
                );
            }

            let now = Utc.with_ymd_and_hms(2030, 1, 1, 12, 6, 0).unwrap();
            let display = store.load_or_recover_display(now).unwrap();
            assert_eq!(display.status, WindowStatus::Fresh);
            assert_eq!(display.weekly_points, Some(2.0));
            assert_eq!(
                display.calibration_id.as_deref(),
                Some(calibration_id.as_str())
            );
            assert_eq!(
                display.window_ends_at,
                Some(now + TimeDelta::hours(4) + TimeDelta::minutes(54))
            );
        }
    }

    fn seed_historical_fixture(connection: &Connection, version: i64, calibration_id: &str) {
        connection
            .execute(
                "INSERT INTO snapshots (
                     source_file, byte_offset, observed_at, used_percent, resets_at,
                     window_minutes, plan_type, affects_meter
                 ) VALUES ('fixture.jsonl', 64, '2030-01-01T12:05:00+00:00', 22.0,
                           '2030-01-08T00:00:00+00:00', 10080, 'plus', 1)",
                [],
            )
            .unwrap();
        if version >= 7 {
            connection
                .execute(
                    "INSERT INTO windows (
                         started_at, ends_at, latest_observed_at, latest_used_percent,
                         calibration_weekly_points, accumulated_weekly_points,
                         last_emitted_milestone, lifecycle, calibration_id,
                         calibration_confidence
                     ) VALUES ('2030-01-01T12:00:00+00:00', '2030-01-01T17:00:00+00:00',
                               '2030-01-01T12:05:00+00:00', 22.0, 15.8, 2.0, NULL,
                               'current', ?1, 'baseline')",
                    [calibration_id],
                )
                .unwrap();
        } else {
            connection
                .execute(
                    "INSERT INTO windows (
                         started_at, ends_at, latest_observed_at, latest_used_percent,
                         calibration_weekly_points, accumulated_weekly_points,
                         last_emitted_milestone, lifecycle
                     ) VALUES ('2030-01-01T12:00:00+00:00', '2030-01-01T17:00:00+00:00',
                               '2030-01-01T12:05:00+00:00', 22.0, 15.8, 2.0, NULL,
                               'current')",
                    [],
                )
                .unwrap();
        }
        if version < 2 {
            return;
        }

        if version >= 9 {
            connection
                .execute(
                    "INSERT INTO rate_limit_observations (
                         source_file, byte_offset, observed_at, model_slug, service_tier,
                         plan_type, schema_fingerprint, compatibility_generation, codex_version
                     ) VALUES ('fixture.jsonl', 64, '2030-01-01T12:05:00+00:00',
                               'gpt-fixture', 'default', 'plus', 'fixture-schema',
                               'fixture-generation', 'codex-cli-fixture')",
                    [],
                )
                .unwrap();
        } else if version >= 7 {
            connection
                .execute(
                    "INSERT INTO rate_limit_observations (
                         source_file, byte_offset, observed_at, model_slug, service_tier,
                         plan_type, schema_fingerprint, compatibility_generation
                     ) VALUES ('fixture.jsonl', 64, '2030-01-01T12:05:00+00:00',
                               'gpt-fixture', 'default', 'plus', 'fixture-schema',
                               'fixture-generation')",
                    [],
                )
                .unwrap();
        } else {
            connection
                .execute(
                    "INSERT INTO rate_limit_observations (
                         source_file, byte_offset, observed_at, model_slug, service_tier,
                         plan_type, schema_fingerprint
                     ) VALUES ('fixture.jsonl', 64, '2030-01-01T12:05:00+00:00',
                               'gpt-fixture', 'default', 'plus', 'fixture-schema')",
                    [],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO diagnostic_events (
                     source_file, byte_offset, observed_at, code, field, message
                 ) VALUES ('fixture.jsonl', 64, '2030-01-01T12:04:00+00:00',
                           'fixture_info', 'rate_limits', 'historical fixture')",
                [],
            )
            .unwrap();
        if version >= 3 {
            connection
                .execute(
                    "INSERT INTO transcript_cursors (
                         source_file, next_offset, updated_at, continuity_marker, generation
                     ) VALUES (?1, 128, '2030-01-01T12:05:00+00:00', ?2, ?3)",
                    params![
                        format!("fixture-v{version}.jsonl"),
                        vec![0xCA_u8, 0xFE],
                        version
                    ],
                )
                .unwrap();
        } else {
            connection
                .execute(
                    "INSERT INTO transcript_cursors (source_file, next_offset, updated_at)
                     VALUES (?1, 128, '2030-01-01T12:05:00+00:00')",
                    [format!("fixture-v{version}.jsonl")],
                )
                .unwrap();
        }
    }
}
