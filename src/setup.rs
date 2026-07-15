use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::{IngestOptions, StateError, StateStore};

const MAX_HISTORY_FILES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryPreview {
    pub sessions_root: PathBuf,
    pub candidate_count: usize,
    pub earliest_modified_at: Option<DateTime<Utc>>,
    pub latest_modified_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub candidate_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryImportSummary {
    pub candidate_count: usize,
    pub imported_file_count: usize,
    pub observation_count: usize,
    pub paired_observation_count: usize,
    pub weekly_only_observation_count: usize,
    pub diagnostic_count: usize,
    pub failed_file_count: usize,
}

pub fn preview_history(sessions_root: &Path) -> io::Result<HistoryPreview> {
    let mut files = Vec::new();
    collect_jsonl(sessions_root, &mut files)?;
    files.sort();
    files.truncate(MAX_HISTORY_FILES);
    let mut modified: Vec<SystemTime> = files
        .iter()
        .filter_map(|path| fs::metadata(path).and_then(|value| value.modified()).ok())
        .collect();
    modified.sort();
    Ok(HistoryPreview {
        sessions_root: sessions_root.to_path_buf(),
        candidate_count: files.len(),
        earliest_modified_at: modified.first().copied().map(DateTime::<Utc>::from),
        latest_modified_at: modified.last().copied().map(DateTime::<Utc>::from),
        candidate_files: files,
    })
}

pub fn import_history(
    store: &mut StateStore,
    preview: &HistoryPreview,
    now: DateTime<Utc>,
) -> HistoryImportSummary {
    let mut summary = HistoryImportSummary {
        candidate_count: preview.candidate_count,
        imported_file_count: 0,
        observation_count: 0,
        paired_observation_count: 0,
        weekly_only_observation_count: 0,
        diagnostic_count: 0,
        failed_file_count: 0,
    };
    let options = IngestOptions {
        now,
        future_tolerance: TimeDelta::minutes(5),
    };
    for path in &preview.candidate_files {
        match store.ingest_transcript(path, &options) {
            Ok(outcome) => {
                summary.imported_file_count += 1;
                summary.observation_count += outcome.batch.observations.len();
                summary.paired_observation_count += outcome
                    .batch
                    .observations
                    .iter()
                    .filter(|item| item.five_hour.is_some() && item.weekly.is_some())
                    .count();
                summary.weekly_only_observation_count += outcome
                    .batch
                    .observations
                    .iter()
                    .filter(|item| item.five_hour.is_none() && item.weekly.is_some())
                    .count();
                summary.diagnostic_count += outcome.batch.diagnostics.len();
            }
            Err(StateError::Ingest(_) | StateError::Io { .. }) => {
                summary.failed_file_count += 1;
            }
            Err(_) => {
                summary.failed_file_count += 1;
            }
        }
    }
    summary
}

fn collect_jsonl(directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    if files.len() >= MAX_HISTORY_FILES {
        return Ok(());
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_jsonl(&path, files)?;
        } else if file_type.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("jsonl")
        {
            files.push(path);
        }
        if files.len() >= MAX_HISTORY_FILES {
            break;
        }
    }
    Ok(())
}
