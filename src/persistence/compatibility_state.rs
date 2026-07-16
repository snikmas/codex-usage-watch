use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::{StateError, StateStore, parse_timestamp};
use crate::calibration::{CalibrationConfidence, CalibrationIdentity, stable_calibration_id};
use crate::compatibility::{CompatibilityCheck, CompatibilityIdentity, CompatibilityResult};

impl StateStore {
    pub fn current_compatibility_identity(
        &self,
        codex_version: Option<&str>,
        model_slug: Option<&str>,
        service_tier: Option<&str>,
    ) -> Result<(CompatibilityIdentity, bool), StateError> {
        type LatestCompatibilityObservation = (
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            String,
        );
        let latest: Option<LatestCompatibilityObservation> = self
            .connection
            .query_row(
                "SELECT plan_type, model_slug, service_tier, schema_fingerprint, observed_at
                 FROM rate_limit_observations
                 ORDER BY observed_at DESC, source_file DESC, byte_offset DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let latest_diagnostic: Option<(String, Option<String>)> = self
            .connection
            .query_row(
                "SELECT code, observed_at FROM diagnostic_events
                 WHERE code LIKE 'unsupported_%' OR code IN ('missing_rate_limits', 'missing_weekly_window')
                 ORDER BY COALESCE(observed_at, '') DESC, rowid DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let diagnostic_is_newer = match (&latest, &latest_diagnostic) {
            (Some((_, _, _, _, observed)), Some((_, Some(diagnostic_at)))) => {
                diagnostic_at >= observed
            }
            (None, Some(_)) => true,
            _ => false,
        };
        let (observed_plan, observed_model, observed_tier, fingerprint) = latest
            .map(|(plan, model, tier, fingerprint, _)| (plan, model, tier, fingerprint))
            .unwrap_or((None, None, None, "unavailable".to_string()));
        let schema_supported = fingerprint != "unavailable" && !diagnostic_is_newer;
        let fingerprint = if diagnostic_is_newer {
            format!(
                "unsupported:{}",
                latest_diagnostic
                    .as_ref()
                    .map(|value| value.0.as_str())
                    .unwrap_or("unknown")
            )
        } else {
            fingerprint
        };
        Ok((
            CompatibilityIdentity {
                codex_version: codex_version.unwrap_or("unknown").to_string(),
                plan_type: observed_plan.unwrap_or_else(|| "unknown".to_string()),
                model_slug: model_slug
                    .map(str::to_string)
                    .or(observed_model)
                    .unwrap_or_else(|| "unknown".to_string()),
                service_tier: service_tier
                    .map(str::to_string)
                    .or(observed_tier)
                    .unwrap_or_else(|| "unknown".to_string()),
                schema_fingerprint: fingerprint,
            },
            schema_supported,
        ))
    }

    pub fn check_compatibility(
        &mut self,
        identity: CompatibilityIdentity,
        schema_supported: bool,
        checked_at: DateTime<Utc>,
    ) -> Result<CompatibilityCheck, StateError> {
        let prior_profile = self.selected_calibration_profile().ok();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = transaction
            .query_row(
                "SELECT result, model_confidence, hook_check, transcript_check,
                        rate_limit_check, projection_check, tracker_version,
                        plugin_version, checked_at
                 FROM compatibility_results WHERE identity_key = ?1",
                [identity.key()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()?
        {
            return Ok(CompatibilityCheck {
                identity,
                result: CompatibilityResult::parse(&existing.0),
                first_seen: false,
                model_confidence: existing.1,
                hook_check: existing.2,
                transcript_check: existing.3,
                rate_limit_check: existing.4,
                projection_check: existing.5,
                tracker_version: existing.6,
                plugin_version: existing.7,
                checked_at: parse_timestamp(&existing.8)?,
            });
        }

        let prior_identity_count: i64 =
            transaction.query_row("SELECT COUNT(*) FROM compatibility_results", [], |row| {
                row.get(0)
            })?;
        let prior_model_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM compatibility_results WHERE model_slug = ?1",
            [identity.model_slug.as_str()],
            |row| row.get(0),
        )?;
        let new_model = prior_identity_count > 0 && prior_model_count == 0;
        let result = if !schema_supported {
            CompatibilityResult::Degraded
        } else if new_model {
            CompatibilityResult::Review
        } else {
            CompatibilityResult::Compatible
        };
        let model_confidence = if new_model {
            "inherited / not validated for this model"
        } else {
            "historical"
        };
        let rate_limit_check = if schema_supported {
            "compatible"
        } else {
            "unsupported; local estimate unavailable"
        };
        let check = CompatibilityCheck {
            identity: identity.clone(),
            result,
            first_seen: true,
            model_confidence: model_confidence.to_string(),
            hook_check: "compatible".to_string(),
            transcript_check: "incremental parser available".to_string(),
            rate_limit_check: rate_limit_check.to_string(),
            projection_check: "display projection v1 supported".to_string(),
            tracker_version: env!("CARGO_PKG_VERSION").to_string(),
            plugin_version: "1".to_string(),
            checked_at,
        };
        transaction.execute(
            "INSERT INTO compatibility_results (
                 identity_key, codex_version, plan_type, model_slug, service_tier,
                 schema_fingerprint, result, model_confidence, hook_check,
                 transcript_check, rate_limit_check, projection_check,
                 tracker_version, plugin_version, checked_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                identity.key(),
                identity.codex_version,
                identity.plan_type,
                identity.model_slug,
                identity.service_tier,
                identity.schema_fingerprint,
                result.as_str(),
                check.model_confidence,
                check.hook_check,
                check.transcript_check,
                check.rate_limit_check,
                check.projection_check,
                check.tracker_version,
                check.plugin_version,
                checked_at.to_rfc3339(),
            ],
        )?;
        let generation = identity.key();
        transaction.execute(
            "UPDATE config_metadata SET current_compatibility_generation = ?1,
                 updated_at = ?2 WHERE singleton = 1",
            params![generation, checked_at.to_rfc3339()],
        )?;
        if result == CompatibilityResult::Review {
            if let Some(previous) = prior_profile {
                if let Some(value) = previous.value {
                    let inherited_identity = CalibrationIdentity {
                        plan_type: identity.plan_type.clone(),
                        model_slug: identity.model_slug.clone(),
                        service_tier: identity.service_tier.clone(),
                        schema_fingerprint: identity.schema_fingerprint.clone(),
                        compatibility_generation: generation,
                    };
                    let calibration_id = stable_calibration_id(
                        &format!("inherited-from:{}", previous.calibration_id),
                        &inherited_identity,
                        value,
                    );
                    transaction.execute(
                        "INSERT OR IGNORE INTO calibration_profiles (
                     calibration_id, identity_key, plan_type, model_slug, service_tier,
                     schema_fingerprint, compatibility_generation, value, confidence,
                     source, evidence_period_start, evidence_period_end, approved_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                        params![
                            calibration_id,
                            inherited_identity.key(),
                            inherited_identity.plan_type,
                            inherited_identity.model_slug,
                            inherited_identity.service_tier,
                            inherited_identity.schema_fingerprint,
                            inherited_identity.compatibility_generation,
                            value,
                            CalibrationConfidence::InheritedUnvalidated.as_str(),
                            format!(
                                "inherited from {} after compatibility change",
                                previous.calibration_id
                            ),
                            previous
                                .evidence_period_start
                                .map(|value| value.to_rfc3339()),
                            previous.evidence_period_end.map(|value| value.to_rfc3339()),
                            checked_at.to_rfc3339(),
                        ],
                    )?;
                }
            }
        }
        transaction.commit()?;
        Ok(check)
    }
}
