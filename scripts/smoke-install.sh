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

# Prove the documented restore sequence against a consistent backup.
"$PREFIX/bin/codex-5h" uninstall --confirm >/dev/null
rm -f "$CODEX_USAGE_WATCH_HOME/state.sqlite3-wal" "$CODEX_USAGE_WATCH_HOME/state.sqlite3-shm"
cp "$TEMP/backup.sqlite3" "$CODEX_USAGE_WATCH_HOME/state.sqlite3"
"$PREFIX/bin/codex-5h" install --confirm >/dev/null
"$PREFIX/bin/codex-5h" doctor >/dev/null

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

# Exercise the exact checksummed archive, including its installer, without the
# source checkout or Cargo being involved in installation.
DIST_DIR="$TEMP/dist" "$ROOT/scripts/package-release.sh"
ARCHIVE="$(find "$TEMP/dist" -maxdepth 1 -name 'codex-usage-watch-*.tar.gz' -print -quit)"
tar -xzf "$ARCHIVE" -C "$TEMP"
PACKAGE_ROOT="$(find "$TEMP" -maxdepth 1 -type d -name 'codex-usage-watch-*-*' -print -quit)"
export PREFIX="$TEMP/package-prefix"
export CODEX_HOME="$TEMP/package-codex-home"
export CODEX_USAGE_WATCH_HOME="$TEMP/package-state"
mkdir -p "$CODEX_HOME"
INSTALL_HOOKS=1 "$PACKAGE_ROOT/scripts/install.sh"
"$PREFIX/bin/codex-5h" setup --skip-import >/dev/null
"$PACKAGE_ROOT/scripts/verify-install.sh"
"$PACKAGE_ROOT/scripts/uninstall.sh" --confirm
test ! -e "$PREFIX/bin/codex-5h"

echo "Source and checksummed-package install, setup, verify, upgrade, backup, rollback, and state retention: PASS"
