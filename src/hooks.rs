use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::{IngestOptions, StateError, StatePaths, StateStore, TrackerConfig, WindowStatus};

const HOOK_TIMEOUT_SECONDS: u64 = 5;
const HOOK_COMMAND_PREFIX: &str = "codex-5h hook ";

#[derive(Debug, Error)]
pub enum HookAdapterError {
    #[error("hook input is invalid: {0}")]
    Input(#[from] serde_json::Error),
    #[error("hook transcript ingestion failed: {0}")]
    Ingest(#[from] crate::ingest::IngestError),
    #[error("hook state update failed: {0}")]
    State(#[from] StateError),
    #[error("hook installation failed at {path}: {source}")]
    InstallIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("refusing to modify hooks without --confirm")]
    ConfirmationRequired,
    #[error("existing hooks file does not contain a JSON object")]
    InvalidHooksFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    Stop,
}

impl HookEvent {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "session-start" => Some(Self::SessionStart),
            "user-prompt-submit" => Some(Self::UserPromptSubmit),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }

    fn wire_name(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Stop => "Stop",
        }
    }

    fn command_name(self) -> &'static str {
        match self {
            Self::SessionStart => "session-start",
            Self::UserPromptSubmit => "user-prompt-submit",
            Self::Stop => "stop",
        }
    }
}

#[derive(Debug, Deserialize)]
struct HookInput {
    hook_event_name: String,
    #[serde(default)]
    transcript_path: Option<PathBuf>,
    #[serde(default, alias = "model_slug")]
    model: Option<String>,
    #[serde(default)]
    service_tier: Option<String>,
    #[serde(default)]
    codex_version: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HookOutput {
    r#continue: bool,
    suppress_output: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_message: Option<String>,
}

pub fn run_hook(
    event: HookEvent,
    input: &str,
    now: DateTime<Utc>,
) -> Result<String, HookAdapterError> {
    let input: HookInput = serde_json::from_str(input)?;
    if input.hook_event_name != event.wire_name() {
        return Err(HookAdapterError::Input(serde_json::Error::io(
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "expected {}, received {}",
                    event.wire_name(),
                    input.hook_event_name
                ),
            ),
        )));
    }

    let outcome = refresh(input.transcript_path.as_deref(), now)?;
    let compatibility = if event == HookEvent::SessionStart {
        let mut store = StateStore::open(TrackerConfig::default())?;
        // Keep the owned environment fallback alive for the identity call.
        let environment_version = std::env::var("CODEX_VERSION").ok();
        let version = input
            .codex_version
            .as_deref()
            .or(environment_version.as_deref());
        let (identity, supported) = store.current_compatibility_identity(
            version,
            input.model.as_deref(),
            input.service_tier.as_deref(),
        )?;
        let check = store.check_compatibility(identity, supported, now)?;
        let report_reason = store
            .maybe_generate_calibration_report(now)?
            .map(|value| value.0);
        Some((check, report_reason))
    } else {
        None
    };
    let system_message = match event {
        HookEvent::SessionStart => {
            let mut message = session_message(&outcome.display);
            if let Some((check, _)) = compatibility.as_ref().filter(|value| value.0.first_seen) {
                message.push_str(&format!(
                    " Compatibility: {:?} for Codex {}, model {}, tier {}; {}.",
                    check.result,
                    check.identity.codex_version,
                    check.identity.model_slug,
                    check.identity.service_tier,
                    check.rate_limit_check
                ));
            }
            if let Some((_, Some(reason))) = compatibility {
                message.push_str(&format!(
                    " Calibration report updated ({reason}); inspect with codex-5h analyze."
                ));
            }
            Some(message)
        }
        HookEvent::UserPromptSubmit => outcome
            .newly_emitted_warnings
            .last()
            .map(|milestone| warning_message(*milestone, &outcome.display)),
        HookEvent::Stop => None,
    };
    serde_json::to_string(&HookOutput {
        r#continue: true,
        suppress_output: system_message.is_none(),
        system_message,
    })
    .map_err(HookAdapterError::Input)
}

fn refresh(
    transcript_path: Option<&Path>,
    now: DateTime<Utc>,
) -> Result<crate::PersistOutcome, HookAdapterError> {
    let config = TrackerConfig::default();
    let mut store = StateStore::open(config)?;
    let paths = transcript_path.map(|path| vec![path.to_path_buf()]);
    let options = IngestOptions {
        now,
        ..IngestOptions::default()
    };
    let Some(paths) = paths else {
        // SessionStart stays cheap. Turn/Stop hooks provide transcript_path and
        // advance the durable cursor; no historical rescan is needed here.
        return store.ingest([], now).map_err(Into::into);
    };
    let mut last = None;
    for path in paths {
        match store.ingest_transcript(&path, &options) {
            Ok(outcome) => last = Some(outcome.persisted),
            Err(StateError::Ingest(crate::ingest::IngestError::TranscriptIo {
                source, ..
            })) if source.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    match last {
        Some(outcome) => Ok(outcome),
        None => store.ingest([], now).map_err(Into::into),
    }
}

fn session_message(display: &crate::DisplayCacheV1) -> String {
    match (
        display.status,
        display.five_hour_estimate_percent,
        display.weekly_points,
    ) {
        (WindowStatus::Fresh, Some(estimate), Some(weekly)) => format!(
            "Usage watch: {:.0}% estimated · week +{weekly:.1}% · continuing normally.",
            estimate.round()
        ),
        (WindowStatus::Stale | WindowStatus::Expired, _, _) => {
            "Usage watch: data is stale; continuing normally.".to_string()
        }
        (WindowStatus::Unknown, _, _) => {
            "Usage watch: usage data is not available yet; continuing normally.".to_string()
        }
        _ => "Usage watch: usage data is incomplete; continuing normally.".to_string(),
    }
}

fn warning_message(milestone: u32, display: &crate::DisplayCacheV1) -> String {
    let estimate = display
        .five_hour_estimate_percent
        .unwrap_or(f64::from(milestone));
    let weekly = display.weekly_points.unwrap_or(0.0);
    if milestone >= 100 {
        format!(
            "Usage watch: historical five-hour allowance exceeded ({estimate:.0}% estimated · week +{weekly:.1}%); continuing normally."
        )
    } else {
        format!(
            "Usage watch: {milestone}% milestone reached ({estimate:.0}% estimated · week +{weekly:.1}%); continuing normally."
        )
    }
}

pub fn install_hooks(confirm: bool) -> Result<PathBuf, HookAdapterError> {
    if !confirm {
        return Err(HookAdapterError::ConfirmationRequired);
    }
    let path = codex_home().join("hooks.json");
    let mut root = read_hooks_root(&path)?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or(HookAdapterError::InvalidHooksFile)?;
    for event in [
        HookEvent::SessionStart,
        HookEvent::UserPromptSubmit,
        HookEvent::Stop,
    ] {
        let groups = hooks
            .entry(event.wire_name())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or(HookAdapterError::InvalidHooksFile)?;
        let command = format!("{HOOK_COMMAND_PREFIX}{}", event.command_name());
        let already_installed = groups.iter().any(|group| {
            group
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|handlers| {
                    handlers.iter().any(|handler| {
                        handler.get("command") == Some(&Value::String(command.clone()))
                    })
                })
        });
        if !already_installed {
            groups.push(json!({
                "hooks": [{
                    "type": "command",
                    "command": command,
                    "timeout": HOOK_TIMEOUT_SECONDS,
                    "statusMessage": "Updating local usage watch"
                }]
            }));
        }
    }
    write_hooks_root(&path, &root)?;
    Ok(path)
}

pub fn uninstall_hooks(confirm: bool) -> Result<PathBuf, HookAdapterError> {
    if !confirm {
        return Err(HookAdapterError::ConfirmationRequired);
    }
    let path = codex_home().join("hooks.json");
    let mut root = read_hooks_root(&path)?;
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(path);
    };
    for event in [
        HookEvent::SessionStart,
        HookEvent::UserPromptSubmit,
        HookEvent::Stop,
    ] {
        let Some(groups) = hooks
            .get_mut(event.wire_name())
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        let command = format!("{HOOK_COMMAND_PREFIX}{}", event.command_name());
        groups.retain(|group| {
            !group
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|handlers| {
                    handlers.iter().any(|handler| {
                        handler.get("command") == Some(&Value::String(command.clone()))
                    })
                })
        });
    }
    write_hooks_root(&path, &root)?;
    Ok(path)
}

fn read_hooks_root(path: &Path) -> Result<Map<String, Value>, HookAdapterError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)?
            .as_object()
            .cloned()
            .ok_or(HookAdapterError::InvalidHooksFile),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(Map::new()),
        Err(source) => Err(HookAdapterError::InstallIo {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn write_hooks_root(path: &Path, root: &Map<String, Value>) -> Result<(), HookAdapterError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| HookAdapterError::InstallIo {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let bytes = serde_json::to_vec_pretty(root)?;
    fs::write(path, bytes).map_err(|source| HookAdapterError::InstallIo {
        path: path.to_path_buf(),
        source,
    })
}

fn codex_home() -> PathBuf {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

pub fn display_path() -> Result<PathBuf, StateError> {
    Ok(StatePaths::resolve(&TrackerConfig::default())?.display)
}
