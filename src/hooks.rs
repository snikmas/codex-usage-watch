use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::{IngestOptions, StateError, StatePaths, StateStore, TrackerConfig, WindowStatus};

const HOOK_TIMEOUT_SECONDS: u64 = 5;
const HOOK_COMMAND_PREFIX: &str = "codex-watch hook ";
const LEGACY_HOOK_COMMAND_PREFIX: &str = "codex-5h hook ";

#[derive(Debug, Error)]
pub enum HookAdapterError {
    #[error("hook configuration is invalid: {0}")]
    Config(#[from] crate::DomainError),
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
    #[error("installed hook configuration is invalid: {0}")]
    InvalidHookConfiguration(String),
    #[error(
        "installed executable path cannot be represented safely in a hook command: {}",
        .0.display()
    )]
    UnsafeExecutablePath(PathBuf),
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
        let mut store = StateStore::open_for_hook(TrackerConfig::from_env()?)?;
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
                    " Calibration report updated ({reason}); inspect with codex-watch analyze."
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
    let config = TrackerConfig::from_env()?;
    let mut store = StateStore::open_for_hook(config)?;
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
    let original = root.clone();
    let executable = env::current_exe().map_err(|source| HookAdapterError::InstallIo {
        path: PathBuf::from("current executable"),
        source,
    })?;
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
        let command = hook_command(&executable, event)?;
        remove_owned_handlers(groups, event);
        groups.push(json!({
            "hooks": [{
                "type": "command",
                "command": command,
                "timeout": HOOK_TIMEOUT_SECONDS,
                "statusMessage": "Updating local usage watch"
            }]
        }));
    }
    if root != original {
        write_hooks_root(&path, &root)?;
    }
    Ok(path)
}

pub fn uninstall_hooks(confirm: bool) -> Result<PathBuf, HookAdapterError> {
    if !confirm {
        return Err(HookAdapterError::ConfirmationRequired);
    }
    let path = codex_home().join("hooks.json");
    let mut root = read_hooks_root(&path)?;
    let original = root.clone();
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
        remove_owned_handlers(groups, event);
    }
    if root != original {
        write_hooks_root(&path, &root)?;
    }
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
    let mut bytes = serde_json::to_vec_pretty(root)?;
    bytes.push(b'\n');
    if let Ok(existing) = fs::read(path) {
        let backup = path.with_file_name(format!(
            "{}.codex-watch.bak",
            path.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("hooks.json")
        ));
        atomic_write(&backup, &existing)?;
    }
    atomic_write(path, &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), HookAdapterError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| HookAdapterError::InstallIo {
            path: parent.to_path_buf(),
            source,
        })?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.flush())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| HookAdapterError::InstallIo {
            path: temporary.path().to_path_buf(),
            source,
        })?;
    temporary
        .persist(path)
        .map_err(|error| HookAdapterError::InstallIo {
            path: path.to_path_buf(),
            source: error.error,
        })?;
    Ok(())
}

fn hook_command(executable: &Path, event: HookEvent) -> Result<String, HookAdapterError> {
    let executable = executable
        .to_str()
        .ok_or_else(|| HookAdapterError::UnsafeExecutablePath(executable.to_path_buf()))?;
    #[cfg(unix)]
    let executable = format!("'{}'", executable.replace('\'', "'\"'\"'"));
    #[cfg(not(unix))]
    let executable = format!("\"{}\"", executable.replace('"', "\\\""));
    Ok(format!("{executable} hook {}", event.command_name()))
}

fn hook_command_matches(command: &str, expected_executable: &Path, event: HookEvent) -> bool {
    hook_command_executable(command, event)
        .is_some_and(|executable| paths_equivalent(&executable, expected_executable))
}

fn hook_command_executable(command: &str, event: HookEvent) -> Option<PathBuf> {
    let suffix = format!(" hook {}", event.command_name());
    let encoded = command.strip_suffix(&suffix)?.trim();
    #[cfg(unix)]
    if encoded.starts_with('\'') && encoded.ends_with('\'') {
        return decode_posix_single_quoted(encoded).map(PathBuf::from);
    }
    let executable = encoded
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(encoded)
        .replace("\\\"", "\"");
    Some(PathBuf::from(executable))
}

#[cfg(unix)]
fn decode_posix_single_quoted(encoded: &str) -> Option<String> {
    let body = encoded.strip_prefix('\'')?.strip_suffix('\'')?;
    let mut decoded = String::with_capacity(body.len());
    let mut remaining = body;
    const ESCAPED_SINGLE_QUOTE: &str = "'\"'\"'";
    while !remaining.is_empty() {
        if let Some(rest) = remaining.strip_prefix(ESCAPED_SINGLE_QUOTE) {
            decoded.push('\'');
            remaining = rest;
            continue;
        }
        let character = remaining.chars().next()?;
        if character == '\'' {
            return None;
        }
        decoded.push(character);
        remaining = &remaining[character.len_utf8()..];
    }
    Some(decoded)
}

fn paths_equivalent(actual: &Path, expected: &Path) -> bool {
    let actual = fs::canonicalize(actual).unwrap_or_else(|_| actual.to_path_buf());
    let expected = fs::canonicalize(expected).unwrap_or_else(|_| expected.to_path_buf());
    if cfg!(windows) {
        actual
            .to_string_lossy()
            .eq_ignore_ascii_case(&expected.to_string_lossy())
    } else {
        actual == expected
    }
}

fn is_owned_handler(handler: &Value, event: HookEvent) -> bool {
    let Some(command) = handler.get("command").and_then(Value::as_str) else {
        return false;
    };
    let current = format!("{HOOK_COMMAND_PREFIX}{}", event.command_name());
    let legacy = format!("{LEGACY_HOOK_COMMAND_PREFIX}{}", event.command_name());
    if command == current || command == legacy {
        return true;
    }
    let Some(executable) = hook_command_executable(command, event) else {
        return false;
    };
    executable
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            matches!(
                value,
                "codex-watch" | "codex-watch.exe" | "codex-5h" | "codex-5h.exe"
            )
        })
}

fn remove_owned_handlers(groups: &mut Vec<Value>, event: HookEvent) {
    for group in groups.iter_mut() {
        if let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) {
            handlers.retain(|handler| !is_owned_handler(handler, event));
        }
    }
    groups.retain(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .is_none_or(|handlers| !handlers.is_empty())
    });
}

pub fn validate_installed_hooks(expected_executable: &Path) -> Result<PathBuf, HookAdapterError> {
    let path = codex_home().join("hooks.json");
    let root = read_hooks_root(&path)?;
    let hooks = root
        .get("hooks")
        .and_then(Value::as_object)
        .ok_or_else(|| HookAdapterError::InvalidHookConfiguration("missing hooks object".into()))?;
    for event in [
        HookEvent::SessionStart,
        HookEvent::UserPromptSubmit,
        HookEvent::Stop,
    ] {
        let matching = hooks
            .get(event.wire_name())
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|group| group.get("hooks").and_then(Value::as_array))
            .flatten()
            .filter(|handler| {
                handler
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|command| {
                        hook_command_matches(command, expected_executable, event)
                    })
            })
            .any(|handler| {
                handler.get("type").and_then(Value::as_str) == Some("command")
                    && handler.get("timeout").and_then(Value::as_u64) == Some(HOOK_TIMEOUT_SECONDS)
            });
        if !matching {
            return Err(HookAdapterError::InvalidHookConfiguration(format!(
                "{} does not contain the expected absolute command",
                event.wire_name()
            )));
        }
    }
    Ok(path)
}

fn codex_home() -> PathBuf {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

pub fn display_path() -> Result<PathBuf, StateError> {
    Ok(StatePaths::resolve(&TrackerConfig::from_env()?)?.display)
}
