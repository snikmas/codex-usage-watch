use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{StateError, StateStore, parse_timestamp, write_json_atomic};
use crate::calibration::{
    CalibrationConfidence, CalibrationIdentity, CalibrationObservation, CalibrationProfile,
    CalibrationReport, build_report, stable_calibration_id,
};
use crate::model::ObservedRateLimitWindow;

impl StateStore {
    pub fn active_calibration(&self) -> f64 {
        self.selected_calibration_profile()
            .ok()
            .and_then(|profile| profile.value)
            .unwrap_or(self.config.calibration_weekly_points())
    }

    pub fn selected_calibration_profile(&self) -> Result<CalibrationProfile, StateError> {
        let identity =
            latest_calibration_identity(&self.connection)?.unwrap_or_else(|| CalibrationIdentity {
                plan_type: "unknown".to_string(),
                model_slug: "unknown".to_string(),
                service_tier: "unknown".to_string(),
                schema_fingerprint: "unavailable".to_string(),
                compatibility_generation: "unknown".to_string(),
            });
        select_calibration_profile(&self.connection, identity)
    }

    pub fn analyze_calibration(
        &mut self,
        analyzed_at: DateTime<Utc>,
    ) -> Result<CalibrationReport, StateError> {
        let observations = load_calibration_observations(&self.connection)?;
        let active_profile = self.selected_calibration_profile()?;
        let preliminary =
            build_report(observations.clone(), active_profile.clone(), analyzed_at, 0);
        let previous: Option<(i64, Option<f64>, i64)> = self
            .connection
            .query_row(
                "SELECT last_sample_count, candidate_value, confirmation_count
                 FROM calibration_analysis_state WHERE identity_key = ?1",
                [preliminary.identity.key()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let previous_count = previous.as_ref().map(|value| value.0 as usize).unwrap_or(0);
        let mut confirmation_count = previous.as_ref().map(|value| value.2 as usize).unwrap_or(0);
        if let Some(candidate) = preliminary.proposed_calibration {
            if preliminary.sample_count > previous_count {
                confirmation_count = match previous.as_ref().and_then(|value| value.1) {
                    Some(old) if ((candidate - old) / old).abs() <= 0.02 => {
                        confirmation_count.saturating_add(1)
                    }
                    _ => 1,
                };
            }
        } else {
            confirmation_count = 0;
        }
        let report = build_report(
            observations,
            active_profile,
            analyzed_at,
            confirmation_count,
        );
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for sample in &report.samples {
            transaction.execute(
                "INSERT OR IGNORE INTO calibration_samples (
                     sample_key, window_started_at, window_ended_at,
                     five_hour_reset_at, weekly_reset_at, five_hour_delta_percent,
                     weekly_delta_points, implied_weekly_points,
                     predicted_five_hour_percent, prediction_error_percent,
                     model_slug, service_tier, plan_type, created_at,
                     identity_key, schema_fingerprint, compatibility_generation,
                     quality, eligible_for_estimator, diagnostic_reason
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                           ?15, ?16, ?17, ?18, ?19, ?20)",
                params![
                    sample.sample_key,
                    sample.window_started_at.to_rfc3339(),
                    sample.window_ended_at.to_rfc3339(),
                    sample.five_hour_reset_at.to_rfc3339(),
                    sample.weekly_reset_at.to_rfc3339(),
                    sample.five_hour_delta_percent,
                    sample.weekly_delta_points,
                    sample.implied_weekly_points,
                    sample.predicted_five_hour_percent,
                    sample.prediction_error_percent,
                    sample.identity.model_slug,
                    sample.identity.service_tier,
                    sample.identity.plan_type,
                    analyzed_at.to_rfc3339(),
                    sample.identity.key(),
                    sample.identity.schema_fingerprint,
                    sample.identity.compatibility_generation,
                    format!("{:?}", sample.quality).to_lowercase(),
                    sample.eligible_for_estimator,
                    sample.diagnostic_reason,
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO calibration_analysis_state (
                 identity_key, last_sample_count, candidate_value,
                 confirmation_count, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(identity_key) DO UPDATE SET
                 last_sample_count = excluded.last_sample_count,
                 candidate_value = excluded.candidate_value,
                 confirmation_count = excluded.confirmation_count,
                 updated_at = excluded.updated_at",
            params![
                report.identity.key(),
                report.sample_count as i64,
                report.proposed_calibration,
                report.drift_confirmation_count as i64,
                analyzed_at.to_rfc3339(),
            ],
        )?;
        transaction.commit()?;
        Ok(report)
    }

    pub fn calibration_sample_count(&self) -> Result<usize, StateError> {
        let count: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM calibration_samples", [], |row| {
                    row.get(0)
                })?;
        Ok(count as usize)
    }

    pub fn maybe_generate_calibration_report(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<Option<(String, CalibrationReport)>, StateError> {
        let report = self.analyze_calibration(now)?;
        let previous: Option<(String, i64)> = self
            .connection
            .query_row(
                "SELECT generated_at, sample_count FROM calibration_reports
                 WHERE identity_key = ?1 ORDER BY generated_at DESC, rowid DESC LIMIT 1",
                [report.identity.key()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let reason = match previous {
            None => "initial",
            Some((_, count)) if report.sample_count >= count as usize + 5 => {
                "five_new_qualifying_windows"
            }
            Some((generated_at, _))
                if now - parse_timestamp(&generated_at)? >= chrono::TimeDelta::days(7) =>
            {
                "weekly"
            }
            Some(_) => return Ok(None),
        };
        let report_json = serde_json::to_string_pretty(&report)?;
        let report_key = format!(
            "{}|{}|{}|{}",
            report.identity.key(),
            reason,
            report.sample_count,
            now.date_naive()
        );
        self.connection.execute(
            "INSERT OR IGNORE INTO calibration_reports (
                 report_key, identity_key, generated_at, reason,
                 sample_count, report_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                report_key,
                report.identity.key(),
                now.to_rfc3339(),
                reason,
                report.sample_count as i64,
                report_json,
            ],
        )?;
        write_json_atomic(
            &self.paths.directory,
            &self.paths.calibration_report,
            &report,
        )?;
        Ok(Some((reason.to_string(), report)))
    }

    pub fn apply_calibration(
        &mut self,
        weekly_points: f64,
        approved_at: DateTime<Utc>,
    ) -> Result<String, StateError> {
        self.config.with_calibration_weekly_points(weekly_points)?;
        let report = self.analyze_calibration(approved_at)?;
        let profile = self.selected_calibration_profile()?;
        if profile.confidence == CalibrationConfidence::Unsupported {
            return Err(StateError::UnsupportedCalibrationIdentity);
        }
        let calibration_id = stable_calibration_id(
            &format!("user-approved:{}", approved_at.to_rfc3339()),
            &profile.identity,
            weekly_points,
        );
        let confidence = if report.confidence == CalibrationConfidence::PersonalValidated {
            CalibrationConfidence::PersonalValidated
        } else {
            CalibrationConfidence::PersonalCandidate
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO calibration_profiles (
                 calibration_id, identity_key, plan_type, model_slug, service_tier,
                 schema_fingerprint, compatibility_generation, value, confidence,
                 source, evidence_period_start, evidence_period_end, approved_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                calibration_id,
                profile.identity.key(),
                profile.identity.plan_type,
                profile.identity.model_slug,
                profile.identity.service_tier,
                profile.identity.schema_fingerprint,
                profile.identity.compatibility_generation,
                weekly_points,
                confidence.as_str(),
                "explicit user approval",
                report.data_period_start.map(|value| value.to_rfc3339()),
                report.data_period_end.map(|value| value.to_rfc3339()),
                approved_at.to_rfc3339()
            ],
        )?;
        transaction.execute(
            "INSERT INTO calibration_applications (
                 applied_at, calibration_weekly_points, sample_count,
                 calibration_id, identity_key
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                approved_at.to_rfc3339(),
                weekly_points,
                report.sample_count as i64,
                calibration_id,
                profile.identity.key()
            ],
        )?;
        transaction.commit()?;
        Ok(calibration_id)
    }
}

fn latest_calibration_identity(
    connection: &Connection,
) -> Result<Option<CalibrationIdentity>, StateError> {
    connection
        .query_row(
            "SELECT COALESCE(plan_type, 'unknown'), COALESCE(model_slug, 'unknown'),
                    COALESCE(service_tier, 'unknown'), schema_fingerprint,
                    compatibility_generation
             FROM rate_limit_observations
             ORDER BY observed_at DESC, source_file DESC, byte_offset DESC LIMIT 1",
            [],
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
        .optional()
        .map_err(StateError::from)
}

pub(super) fn select_calibration_profile(
    connection: &Connection,
    identity: CalibrationIdentity,
) -> Result<CalibrationProfile, StateError> {
    type ProfileRow = (
        String,
        f64,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
    );
    let row: Option<ProfileRow> = connection
        .query_row(
            "SELECT calibration_id, value, confidence, source,
                    evidence_period_start, evidence_period_end, approved_at
             FROM calibration_profiles WHERE identity_key = ?1
             ORDER BY approved_at DESC, rowid DESC LIMIT 1",
            [identity.key()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()?;
    if let Some((id, value, confidence, source, start, end, approved)) = row {
        return Ok(CalibrationProfile {
            calibration_id: id,
            identity,
            value: Some(value),
            confidence: CalibrationConfidence::parse(&confidence),
            source,
            evidence_period_start: start.as_deref().map(parse_timestamp).transpose()?,
            evidence_period_end: end.as_deref().map(parse_timestamp).transpose()?,
            approved_at: Some(parse_timestamp(&approved)?),
        });
    }
    if identity.supports_plus_baseline() {
        Ok(CalibrationProfile::plus_baseline(identity))
    } else {
        Ok(CalibrationProfile::unsupported(identity))
    }
}

fn load_calibration_observations(
    connection: &Connection,
) -> Result<Vec<CalibrationObservation>, StateError> {
    type Row = (
        String,
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        String,
        Option<f64>,
        Option<String>,
        Option<f64>,
        Option<String>,
    );
    let mut statement = connection.prepare(
        "SELECT o.source_file, o.byte_offset, o.observed_at, o.model_slug,
                o.service_tier, o.plan_type, o.schema_fingerprint,
                o.compatibility_generation,
                MAX(CASE WHEN w.window_kind = 'five_hour' THEN w.used_percent END),
                MAX(CASE WHEN w.window_kind = 'five_hour' THEN w.resets_at END),
                MAX(CASE WHEN w.window_kind = 'weekly' THEN w.used_percent END),
                MAX(CASE WHEN w.window_kind = 'weekly' THEN w.resets_at END)
         FROM rate_limit_observations o
         LEFT JOIN observed_rate_limit_windows w
           ON w.source_file = o.source_file AND w.byte_offset = o.byte_offset
         GROUP BY o.source_file, o.byte_offset
         ORDER BY o.observed_at, o.source_file, o.byte_offset",
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
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (
            source,
            offset,
            observed_at,
            model_slug,
            service_tier,
            plan_type,
            schema_fingerprint,
            compatibility_generation,
            five_used,
            five_reset,
            weekly_used,
            weekly_reset,
        ): Row = row?;
        let five_reset = five_reset.as_deref().map(parse_timestamp).transpose()?;
        let weekly_reset = weekly_reset.as_deref().map(parse_timestamp).transpose()?;
        result.push(CalibrationObservation {
            source_key: format!("{source}:{offset}"),
            observed_at: parse_timestamp(&observed_at)?,
            five_hour: five_used.map(|used_percent| ObservedRateLimitWindow {
                used_percent,
                window_minutes: crate::model::FIVE_HOUR_WINDOW_MINUTES,
                resets_at: five_reset,
            }),
            weekly: weekly_used.map(|used_percent| ObservedRateLimitWindow {
                used_percent,
                window_minutes: crate::model::WEEKLY_WINDOW_MINUTES,
                resets_at: weekly_reset,
            }),
            identity: CalibrationIdentity {
                plan_type: plan_type.unwrap_or_else(|| "unknown".to_string()),
                model_slug: model_slug.unwrap_or_else(|| "unknown".to_string()),
                service_tier: service_tier.unwrap_or_else(|| "unknown".to_string()),
                schema_fingerprint,
                compatibility_generation,
            },
        });
    }
    Ok(result)
}
