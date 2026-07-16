#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != "--confirm" ]]; then
  echo "Refusing to uninstall without --confirm" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin/codex-5h"
CODEX_HOME="${CODEX_HOME:-$HOME/.codex}"
HOOKS_FILE="$CODEX_HOME/hooks.json"
if [[ -x "$BIN" ]]; then
  "$BIN" uninstall --confirm
  rm -f "$BIN"
  echo "Removed the codex-5h binary and its hook entries."
elif [[ -x "$ROOT/codex-5h" && -f "$ROOT/BUILD-INFO.json" && -f "$ROOT/SBOM.spdx.json" ]]; then
  "$ROOT/codex-5h" uninstall --confirm
  echo "The installed codex-5h binary was already absent. Removed its hook entries with the verified archive binary."
else
  if python3 - "$HOOKS_FILE" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
if not path.exists():
    raise SystemExit(0)
try:
    root = json.loads(path.read_text(encoding="utf-8"))
except (OSError, UnicodeError, json.JSONDecodeError):
    raise SystemExit(11)

events = {
    "SessionStart": "session-start",
    "UserPromptSubmit": "user-prompt-submit",
    "Stop": "stop",
}

def decode_posix_word(encoded):
    if not (encoded.startswith("'") and encoded.endswith("'")):
        return encoded.strip('"').replace('\\\\"', '"')
    body = encoded[1:-1]
    escaped_quote = "'\"'\"'"
    decoded = []
    while body:
        if body.startswith(escaped_quote):
            decoded.append("'")
            body = body[len(escaped_quote):]
        elif body.startswith("'"):
            return None
        else:
            decoded.append(body[0])
            body = body[1:]
    return "".join(decoded)

def owned(command, event):
    if command == f"codex-5h hook {event}":
        return True
    suffix = f" hook {event}"
    if not isinstance(command, str) or not command.endswith(suffix):
        return False
    executable = decode_posix_word(command[:-len(suffix)].strip())
    if executable is None:
        return False
    return pathlib.PurePosixPath(executable).name in {"codex-5h", "codex-5h.exe"}

hooks = root.get("hooks", {}) if isinstance(root, dict) else {}
for wire_event, command_event in events.items():
    groups = hooks.get(wire_event, []) if isinstance(hooks, dict) else []
    for group in groups if isinstance(groups, list) else []:
        handlers = group.get("hooks", []) if isinstance(group, dict) else []
        for handler in handlers if isinstance(handlers, list) else []:
            if isinstance(handler, dict) and owned(handler.get("command"), command_event):
                raise SystemExit(10)
raise SystemExit(0)
PY
  then
    echo "The codex-5h binary was already absent and no Codex Usage Watch hooks were present."
  else
    status=$?
    echo "Partial cleanup only: the codex-5h binary is absent, so hook configuration was not changed." >&2
    if [[ "$status" == "10" ]]; then
      echo "Codex Usage Watch hook entries are still present in $HOOKS_FILE." >&2
    else
      echo "Hook state could not be verified safely at $HOOKS_FILE." >&2
    fi
    echo "Recovery: download the matching release archive and SHA256SUMS, verify the checksum, extract it, then rerun PREFIX=\"$PREFIX\" CODEX_HOME=\"$CODEX_HOME\" <archive>/scripts/uninstall.sh --confirm." >&2
    echo "No hook file or tracker state was modified." >&2
    exit 5
  fi
fi

echo "Tracker state was preserved. Remove it manually only after making a backup."
