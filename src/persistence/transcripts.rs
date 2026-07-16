use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::{
    PersistOutcome, PersistTranscriptOutcome, StateError, StateStore, insert_snapshots,
    rebuild_windows,
};
use crate::ingest::{IngestBatch, IngestOptions, TranscriptCursor, ingest_transcript};
use crate::model::{IngestDiagnostic, UsageObservation};

impl StateStore {
    pub fn transcript_cursor(&self, path: &Path) -> Result<TranscriptCursor, StateError> {
        Ok(self.transcript_cursor_state(path)?.0)
    }

    fn transcript_cursor_state(
        &self,
        path: &Path,
    ) -> Result<(TranscriptCursor, Vec<u8>, u64), StateError> {
        let canonical = fs::canonicalize(path).map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let row: Option<(i64, Vec<u8>, i64)> = self
            .connection
            .query_row(
                "SELECT next_offset, continuity_marker, generation
                 FROM transcript_cursors WHERE source_file = ?1",
                [canonical.to_string_lossy().as_ref()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let (offset, marker, generation) = row.unwrap_or((0, Vec::new(), 0));
        Ok((
            TranscriptCursor {
                next_offset: u64::try_from(offset).unwrap_or(0),
            },
            marker,
            u64::try_from(generation).unwrap_or(0),
        ))
    }

    /// Reads only the uncommitted suffix of a transcript and atomically stores
    /// observations, diagnostics, weekly snapshots, and the matching cursor.
    pub fn ingest_transcript(
        &mut self,
        path: &Path,
        options: &IngestOptions,
    ) -> Result<PersistTranscriptOutcome, StateError> {
        let (mut cursor, saved_marker, mut generation) = self.transcript_cursor_state(path)?;
        if cursor.next_offset > 0 && !saved_marker.is_empty() {
            let current_marker = transcript_continuity_marker(path, cursor.next_offset)?;
            if current_marker.as_deref() != Some(saved_marker.as_slice()) {
                cursor.next_offset = 0;
                generation = generation.saturating_add(1);
            }
        }
        let mut batch = ingest_transcript(path, cursor, options)?;
        apply_transcript_generation(&mut batch, generation);
        let marker = transcript_continuity_marker(path, batch.next_offset)?.unwrap_or_default();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted_snapshots = insert_snapshots(&transaction, &batch.snapshots)?;
        let inserted_observations = insert_observations(&transaction, &batch.observations)?;
        let inserted_diagnostics = insert_diagnostics(&transaction, &batch.diagnostics)?;
        transaction.execute(
            "INSERT INTO transcript_cursors (
                 source_file, next_offset, updated_at, continuity_marker, generation
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(source_file) DO UPDATE SET
                 next_offset = excluded.next_offset,
                 updated_at = excluded.updated_at,
                 continuity_marker = excluded.continuity_marker,
                 generation = excluded.generation",
            params![
                batch.source_file.to_string_lossy(),
                i64::try_from(batch.next_offset).unwrap_or(i64::MAX),
                options.now.to_rfc3339(),
                marker,
                i64::try_from(generation).unwrap_or(i64::MAX),
            ],
        )?;
        let replay = rebuild_windows(&transaction, &self.config, options.now)?;
        transaction.commit()?;
        let display = self.regenerate_display(options.now)?;
        Ok(PersistTranscriptOutcome {
            batch,
            inserted_observations,
            inserted_diagnostics,
            persisted: PersistOutcome {
                inserted_snapshots,
                newly_emitted_warnings: replay.new_warnings,
                display,
            },
        })
    }
}

fn transcript_continuity_marker(path: &Path, offset: u64) -> Result<Option<Vec<u8>>, StateError> {
    let mut file = fs::File::open(path).map_err(|source| StateError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let length = file
        .metadata()
        .map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    if offset > length {
        return Ok(None);
    }

    const SAMPLE: u64 = 128;
    let first_len = offset.min(SAMPLE) as usize;
    let tail_start = offset.saturating_sub(SAMPLE);
    let tail_len = (offset - tail_start) as usize;
    let mut marker = Vec::with_capacity(16 + first_len + tail_len);
    marker.extend_from_slice(&offset.to_le_bytes());
    let mut buffer = vec![0; first_len];
    file.read_exact(&mut buffer)
        .map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    marker.extend_from_slice(&buffer);
    file.seek(SeekFrom::Start(tail_start))
        .and_then(|_| {
            buffer.resize(tail_len, 0);
            file.read_exact(&mut buffer)
        })
        .map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    marker.extend_from_slice(&buffer);
    Ok(Some(marker))
}

fn apply_transcript_generation(batch: &mut IngestBatch, generation: u64) {
    let logical_source = PathBuf::from(format!(
        "{}#codex-usage-watch-generation={generation}",
        batch.source_file.to_string_lossy()
    ));
    for snapshot in &mut batch.snapshots {
        snapshot.id.source_file = logical_source.clone();
    }
    for observation in &mut batch.observations {
        observation.id.source_file = logical_source.clone();
    }
    for diagnostic in &mut batch.diagnostics {
        diagnostic.id.source_file = logical_source.clone();
    }
}

fn insert_observations(
    transaction: &Transaction<'_>,
    observations: &[UsageObservation],
) -> Result<usize, StateError> {
    let mut inserted = 0;
    let compatibility_generation: String = transaction.query_row(
        "SELECT current_compatibility_generation FROM config_metadata WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    for observation in observations {
        inserted += transaction.execute(
            "INSERT OR IGNORE INTO rate_limit_observations (
                 source_file, byte_offset, observed_at, model_slug, service_tier,
                 plan_type, schema_fingerprint, compatibility_generation,
                 codex_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                observation.id.source_file.to_string_lossy(),
                i64::try_from(observation.id.byte_offset).unwrap_or(i64::MAX),
                observation.observed_at.to_rfc3339(),
                observation.model_slug,
                observation.service_tier,
                observation.plan_type,
                observation.schema_fingerprint,
                observation
                    .codex_version
                    .as_ref()
                    .map(|version| format!("codex-version:{version}"))
                    .unwrap_or_else(|| compatibility_generation.clone()),
                observation.codex_version,
            ],
        )?;
        for (kind, window) in [
            ("five_hour", observation.five_hour.as_ref()),
            ("weekly", observation.weekly.as_ref()),
        ] {
            if let Some(window) = window {
                transaction.execute(
                    "INSERT OR IGNORE INTO observed_rate_limit_windows (
                         source_file, byte_offset, window_kind, used_percent,
                         window_minutes, resets_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        observation.id.source_file.to_string_lossy(),
                        i64::try_from(observation.id.byte_offset).unwrap_or(i64::MAX),
                        kind,
                        window.used_percent,
                        window.window_minutes,
                        window.resets_at.map(|value| value.to_rfc3339()),
                    ],
                )?;
            }
        }
    }
    Ok(inserted)
}

fn insert_diagnostics(
    transaction: &Transaction<'_>,
    diagnostics: &[IngestDiagnostic],
) -> Result<usize, StateError> {
    let mut inserted = 0;
    for diagnostic in diagnostics {
        inserted += transaction.execute(
            "INSERT OR IGNORE INTO diagnostic_events (
                 source_file, byte_offset, observed_at, code, field, message
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                diagnostic.id.source_file.to_string_lossy(),
                i64::try_from(diagnostic.id.byte_offset).unwrap_or(i64::MAX),
                diagnostic.observed_at.map(|value| value.to_rfc3339()),
                diagnostic.code,
                diagnostic.field.as_deref().unwrap_or(""),
                diagnostic.message,
            ],
        )?;
    }
    Ok(inserted)
}
