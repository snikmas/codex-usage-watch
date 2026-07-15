#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT

export PREFIX="$TEMP/prefix"
export CODEX_HOME="$TEMP/codex-home"
export CODEX_USAGE_WATCH_HOME="$TEMP/state"
export PATH="$PREFIX/bin:$PATH"
mkdir -p "$CODEX_HOME"
printf '%s\n' '{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"other-hook"}]}]}}' >"$CODEX_HOME/hooks.json"

INSTALL_HOOKS=1 "$ROOT/scripts/install.sh"
"$PREFIX/bin/codex-5h" setup --skip-import >/dev/null
printf '%s' '{"hook_event_name":"SessionStart","transcript_path":null,"codex_version":"smoke"}' \
  | "$PREFIX/bin/codex-5h" hook session-start >/dev/null
"$ROOT/scripts/verify-install.sh"
"$PREFIX/bin/codex-5h" doctor >/dev/null
"$PREFIX/bin/codex-5h" history --json >/dev/null
"$PREFIX/bin/codex-5h" reset --confirm >/dev/null
"$ROOT/scripts/backup-state.sh" "$TEMP/backup.sqlite3"

# Upgrade is the same reproducible, state-preserving install over an existing version.
"$ROOT/scripts/install.sh"
test -f "$CODEX_USAGE_WATCH_HOME/state.sqlite3"
"$ROOT/scripts/uninstall.sh" --confirm
test ! -e "$PREFIX/bin/codex-5h"
test -f "$CODEX_USAGE_WATCH_HOME/state.sqlite3"
test -f "$TEMP/backup.sqlite3"
python3 - "$CODEX_HOME/hooks.json" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1], encoding="utf-8"))
encoded = json.dumps(data)
assert "other-hook" in encoded
assert "codex-5h hook" not in encoded
PY

echo "Clean install, setup skip, verify, upgrade, backup, rollback, and state retention: PASS"
