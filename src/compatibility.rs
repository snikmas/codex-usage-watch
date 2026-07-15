use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityIdentity {
    pub codex_version: String,
    pub plan_type: String,
    pub model_slug: String,
    pub service_tier: String,
    pub schema_fingerprint: String,
}

impl CompatibilityIdentity {
    pub fn key(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            self.codex_version,
            self.plan_type,
            self.model_slug,
            self.service_tier,
            self.schema_fingerprint
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityResult {
    Compatible,
    Review,
    Degraded,
}

impl CompatibilityResult {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Compatible => "compatible",
            Self::Review => "review",
            Self::Degraded => "degraded",
        }
    }

    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "compatible" => Self::Compatible,
            "review" => Self::Review,
            _ => Self::Degraded,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityCheck {
    pub identity: CompatibilityIdentity,
    pub result: CompatibilityResult,
    pub first_seen: bool,
    pub model_confidence: String,
    pub hook_check: String,
    pub transcript_check: String,
    pub rate_limit_check: String,
    pub projection_check: String,
    pub tracker_version: String,
    pub plugin_version: String,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseMetadata {
    pub checked_at: DateTime<Utc>,
    pub tag_name: String,
    pub html_url: String,
}

#[derive(Debug, Error)]
pub enum ReleaseMetadataError {
    #[error("release metadata I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("release metadata is invalid: {0}")]
    Invalid(String),
    #[error("curl failed or is unavailable")]
    Curl,
}

pub fn cached_release_metadata(
    state_directory: &Path,
    now: DateTime<Utc>,
    refresh: bool,
) -> Result<Option<ReleaseMetadata>, ReleaseMetadataError> {
    let cache_path = state_directory.join("release-metadata.json");
    if let Ok(bytes) = fs::read(&cache_path) {
        if let Ok(cached) = serde_json::from_slice::<ReleaseMetadata>(&bytes) {
            if now - cached.checked_at < TimeDelta::hours(24) || !refresh {
                return Ok(Some(cached));
            }
        }
    }
    if !refresh {
        return Ok(None);
    }

    let bytes = if let Some(path) = std::env::var_os("CODEX_USAGE_WATCH_RELEASE_METADATA_FILE") {
        let path = PathBuf::from(path);
        fs::read(&path).map_err(|source| ReleaseMetadataError::Io { path, source })?
    } else {
        let output = Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--max-time",
                "5",
                "--header",
                "Accept: application/vnd.github+json",
                "https://api.github.com/repos/openai/codex/releases/latest",
            ])
            .output()
            .map_err(|_| ReleaseMetadataError::Curl)?;
        if !output.status.success() || output.stdout.len() > 1_000_000 {
            return Err(ReleaseMetadataError::Curl);
        }
        output.stdout
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| ReleaseMetadataError::Invalid(error.to_string()))?;
    let tag_name = value
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ReleaseMetadataError::Invalid("tag_name is missing".to_string()))?;
    let html_url = value
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ReleaseMetadataError::Invalid("html_url is missing".to_string()))?;
    if !html_url.starts_with("https://github.com/openai/codex/releases/") {
        return Err(ReleaseMetadataError::Invalid(
            "html_url is not an official Codex release URL".to_string(),
        ));
    }
    let metadata = ReleaseMetadata {
        checked_at: now,
        tag_name: tag_name.to_string(),
        html_url: html_url.to_string(),
    };
    fs::create_dir_all(state_directory).map_err(|source| ReleaseMetadataError::Io {
        path: state_directory.to_path_buf(),
        source,
    })?;
    let encoded = serde_json::to_vec_pretty(&metadata)
        .map_err(|error| ReleaseMetadataError::Invalid(error.to_string()))?;
    fs::write(&cache_path, encoded).map_err(|source| ReleaseMetadataError::Io {
        path: cache_path,
        source,
    })?;
    Ok(Some(metadata))
}
