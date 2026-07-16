# Privacy and threat model

Codex Usage Watch runs as the current local user. It reads only local Codex
transcript JSONL files selected by an explicit setup import, a lifecycle hook's
`transcript_path`, or the bounded `refresh` command.

## Fields read and retained

The parser examines the event envelope, timestamp, event type, rate-limit
windows (`used_percent`, `window_minutes`, and reset time), plan type, model,
service tier, Codex version, and enough schema shape to detect compatibility.
It retains normalized observations, calibration evidence, diagnostic codes,
incremental byte cursors, and the canonical transcript file path used to make
those cursors safe and idempotent. The path can contain a local account name;
it stays in the local SQLite database.

## Fields ignored and never retained

Prompt text, model responses, reasoning, tool names and arguments, command
output, source code, arbitrary event payloads, and unrelated JSON fields are
not copied into tracker state. Malformed records produce bounded diagnostic
metadata rather than retaining their raw contents.

## Network behavior

Normal tracking, status, analysis, setup, hooks, backup, and refresh are
offline. The optional `doctor --compat --refresh-releases` path contacts the
official GitHub releases API and caches allowlisted release fields. It sends no
transcript, state, prompt, project, model, usage, or calibration data. Returned
prose is treated only as data and cannot change calibration or execute commands.

## Trust boundary and local control

This tool cannot protect its output from another process running as the same
user. Such a process can edit or delete hooks, thresholds, state, backups, or
`display.json`. Accordingly, the meter is for attention only: it is not an
OpenAI quota, a billing record, a task-cost predictor, or an enforcement
boundary. Every hook fails open and Codex continues above 100%, with stale data,
or when the tracker is unavailable.

Users can inspect or disable hooks in `$CODEX_HOME/hooks.json`, remove this
tool's entries with `codex-5h uninstall --confirm`, choose state location with
`CODEX_USAGE_WATCH_HOME`, and delete state after making any desired backup.

## Local file permissions

On Unix, the tracker-owned state directory is created and repaired to `0700`.
The SQLite database and its WAL/SHM sidecars, `display.json`, calibration and
release reports, and tracker-created backups are `0600`. Startup repairs older
permissive tracker-owned files without deleting or rewriting their contents and
does not change the mode of the user-selected parent directory.

These mode bits are a Unix control only. Windows does not use this guarantee;
Windows installation remains unsupported for this beta. A process running as
the same user can still read or change tracker state.

`doctor --json` and `doctor --support-bundle FILE --confirm` exclude transcript
paths, local state paths, account names, raw observations, prompts, responses,
source code, and databases. Review any diagnostic file before attaching it to
an issue, as you would any locally generated file.
