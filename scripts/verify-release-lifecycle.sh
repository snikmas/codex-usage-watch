#!/usr/bin/env bash
set -euo pipefail
umask 022

if [[ $# -ne 2 && $# -ne 4 ]]; then
  echo "usage: scripts/verify-release-lifecycle.sh ARCHIVE SHA256SUMS [PRIOR_ARCHIVE PRIOR_SHA256SUMS]" >&2
  exit 2
fi

ARCHIVE="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
CHECKSUMS="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
test -f "$ARCHIVE"
test -f "$CHECKSUMS"
PRIOR_ARCHIVE=""
PRIOR_CHECKSUMS=""
if [[ $# -eq 4 ]]; then
  PRIOR_ARCHIVE="$(cd "$(dirname "$3")" && pwd)/$(basename "$3")"
  PRIOR_CHECKSUMS="$(cd "$(dirname "$4")" && pwd)/$(basename "$4")"
  test -f "$PRIOR_ARCHIVE"
  test -f "$PRIOR_CHECKSUMS"
fi

checksum() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

verify_checksum() {
  local archive="$1"
  local checksums="$2"
  local name expected actual
  name="$(basename "$archive")"
  expected="$(awk -v name="$name" '$2 == name || $2 == "*" name {print $1}' "$checksums")"
  if [[ -z "$expected" || "$(printf '%s\n' "$expected" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "checksum file must contain exactly one entry for $name" >&2
    exit 2
  fi
  actual="$(checksum "$archive")"
  if [[ "$actual" != "$expected" ]]; then
    echo "checksum mismatch for $name" >&2
    exit 2
  fi
  printf '%s\n' "$actual"
}

ARCHIVE_NAME="$(basename "$ARCHIVE")"
ACTUAL="$(verify_checksum "$ARCHIVE" "$CHECKSUMS")"

case "$(uname -s):$(uname -m)" in
  Linux:x86_64) EXPECTED_TARGET="x86_64-unknown-linux-gnu" ;;
  Darwin:arm64) EXPECTED_TARGET="aarch64-apple-darwin" ;;
  *)
    echo "unsupported acceptance platform: $(uname -s) $(uname -m)" >&2
    exit 2
    ;;
esac
case "$ARCHIVE_NAME" in
  *-"$EXPECTED_TARGET".tar.gz) ;;
  *)
    echo "artifact target does not match this machine: expected $EXPECTED_TARGET" >&2
    exit 2
    ;;
esac
if [[ -n "$PRIOR_ARCHIVE" ]]; then
  case "$(basename "$PRIOR_ARCHIVE")" in
    *-"$EXPECTED_TARGET".tar.gz) ;;
    *)
      echo "prior artifact target does not match this machine: expected $EXPECTED_TARGET" >&2
      exit 2
      ;;
  esac
  verify_checksum "$PRIOR_ARCHIVE" "$PRIOR_CHECKSUMS" >/dev/null
fi

TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT
tar -xzf "$ARCHIVE" -C "$TEMP"
PACKAGE_ROOT="$(find "$TEMP" -mindepth 1 -maxdepth 1 -type d -name 'codex-usage-watch-*' -print -quit)"
test -n "$PACKAGE_ROOT"
PRIOR_ROOT=""
if [[ -n "$PRIOR_ARCHIVE" ]]; then
  mkdir -p "$TEMP/prior"
  tar -xzf "$PRIOR_ARCHIVE" -C "$TEMP/prior"
  PRIOR_ROOT="$(find "$TEMP/prior" -mindepth 1 -maxdepth 1 -type d -name 'codex-usage-watch-*' -print -quit)"
  test -n "$PRIOR_ROOT"
  test -x "$PRIOR_ROOT/codex-5h"
fi

python3 - "$PACKAGE_ROOT/BUILD-INFO.json" "$EXPECTED_TARGET" "$ACTUAL" <<'PY'
import json
import pathlib
import sys

build = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
assert build["target"] == sys.argv[2], build
assert build["version"], build
assert len(sys.argv[3]) == 64
PY
python3 -m json.tool "$PACKAGE_ROOT/SBOM.spdx.json" >/dev/null
bash "$PACKAGE_ROOT/scripts/check-package-docs.sh" "$PACKAGE_ROOT"

export PREFIX="$TEMP/space path/使用/prefix"
export CODEX_HOME="$TEMP/space path/使用/codex home"
export CODEX_USAGE_WATCH_HOME="$TEMP/space path/使用/state"
mkdir -p "$CODEX_HOME"
printf '%s\n' '{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"unrelated-hook"}]}]}}' >"$CODEX_HOME/hooks.json"

PRIOR_SCHEMA=""
if [[ -n "$PRIOR_ROOT" ]]; then
  INSTALL_HOOKS=1 bash "$PRIOR_ROOT/scripts/install.sh" >/dev/null
  "$PREFIX/bin/codex-5h" refresh >/dev/null
  PRIOR_SCHEMA="$(python3 - "$CODEX_USAGE_WATCH_HOME/state.sqlite3" <<'PY'
import sqlite3
import sys

with sqlite3.connect(sys.argv[1]) as connection:
    print(connection.execute("PRAGMA user_version").fetchone()[0])
PY
)"
  cp "$CODEX_USAGE_WATCH_HOME/state.sqlite3" "$TEMP/prior-state.sqlite3"
  bash "$PRIOR_ROOT/scripts/uninstall.sh" --confirm >/dev/null
fi

INSTALL_HOOKS=1 bash "$PACKAGE_ROOT/scripts/install.sh" >/dev/null
BIN="$PREFIX/bin/codex-watch"
"$BIN" setup --skip-import >/dev/null
"$BIN" status >/dev/null
"$BIN" status --json | python3 -m json.tool >/dev/null
"$BIN" refresh >/dev/null
"$BIN" history >/dev/null
"$BIN" history --json | python3 -m json.tool >/dev/null
"$BIN" analyze >/dev/null
"$BIN" doctor >/dev/null
if [[ -n "$PRIOR_SCHEMA" ]]; then
  CURRENT_SCHEMA="$(python3 - "$CODEX_USAGE_WATCH_HOME/state.sqlite3" <<'PY'
import sqlite3
import sys

with sqlite3.connect(sys.argv[1]) as connection:
    print(connection.execute("PRAGMA user_version").fetchone()[0])
PY
)"
  test "$CURRENT_SCHEMA" -gt "$PRIOR_SCHEMA"
fi

for event in session-start user-prompt-submit stop; do
  case "$event" in
    session-start) wire_event="SessionStart" ;;
    user-prompt-submit) wire_event="UserPromptSubmit" ;;
    stop) wire_event="Stop" ;;
  esac
  printf '%s' "{\"hook_event_name\":\"$wire_event\",\"transcript_path\":null,\"codex_version\":\"acceptance\"}" \
    | "$BIN" hook "$event" | python3 -m json.tool >/dev/null
done

if [[ "$(uname -s)" == "Darwin" ]]; then
  DEFAULT_HOME="$TEMP/default home"
  mkdir -p "$DEFAULT_HOME/.codex"
  env -u CODEX_USAGE_WATCH_HOME HOME="$DEFAULT_HOME" CODEX_HOME="$DEFAULT_HOME/.codex" \
    "$BIN" refresh >/dev/null
  DEFAULT_STATE="$DEFAULT_HOME/Library/Application Support/dev.codex-usage-watch.codex-usage-watch"
  test -f "$DEFAULT_STATE/state.sqlite3"
  python3 - "$DEFAULT_STATE" <<'PY'
import pathlib
import stat
import sys

state = pathlib.Path(sys.argv[1])
assert stat.S_IMODE(state.stat().st_mode) == 0o700
for path in (state / "state.sqlite3", state / "display.json"):
    assert stat.S_IMODE(path.stat().st_mode) == 0o600, path
PY
fi

bash "$PACKAGE_ROOT/scripts/backup-state.sh" "$TEMP/backup.sqlite3" >/dev/null

if [[ -n "$PRIOR_ROOT" ]]; then
  # Roll back both binary and state, prove the prior release still opens its
  # original schema, then upgrade once more to the candidate.
  bash "$PACKAGE_ROOT/scripts/uninstall.sh" --confirm >/dev/null
  rm -f "$CODEX_USAGE_WATCH_HOME/state.sqlite3-wal" "$CODEX_USAGE_WATCH_HOME/state.sqlite3-shm"
  cp "$TEMP/prior-state.sqlite3" "$CODEX_USAGE_WATCH_HOME/state.sqlite3"
  INSTALL_HOOKS=1 bash "$PRIOR_ROOT/scripts/install.sh" >/dev/null
  "$PREFIX/bin/codex-5h" doctor >/dev/null
  bash "$PRIOR_ROOT/scripts/uninstall.sh" --confirm >/dev/null
  INSTALL_HOOKS=1 bash "$PACKAGE_ROOT/scripts/install.sh" >/dev/null
  "$BIN" refresh >/dev/null
  "$BIN" doctor >/dev/null
fi

bash "$PACKAGE_ROOT/scripts/uninstall.sh" --confirm >/dev/null
test ! -e "$BIN"
test -f "$CODEX_USAGE_WATCH_HOME/state.sqlite3"
test -f "$TEMP/backup.sqlite3"

python3 - "$CODEX_HOME/hooks.json" "$CODEX_USAGE_WATCH_HOME" "$TEMP/backup.sqlite3" <<'PY'
import json
import pathlib
import stat
import sys

hooks = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
encoded = json.dumps(hooks)
assert "unrelated-hook" in encoded
assert "codex-watch hook" not in encoded

state = pathlib.Path(sys.argv[2])
backup = pathlib.Path(sys.argv[3])
assert stat.S_IMODE(state.stat().st_mode) == 0o700
for path in (state / "state.sqlite3", state / "display.json", backup):
    assert stat.S_IMODE(path.stat().st_mode) == 0o600, path
PY

echo "Exact-artifact lifecycle: PASS ($EXPECTED_TARGET, sha256 $ACTUAL)"
