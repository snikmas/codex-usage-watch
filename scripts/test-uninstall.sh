#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT

checksum() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

cargo build --manifest-path "$ROOT/Cargo.toml" --locked
TEST_BIN="$ROOT/target/debug/codex-watch"

assert_hook_state() {
  python3 - "$1" "$2" "$3" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1], encoding="utf-8"))
encoded = json.dumps(data)
assert ("codex-watch" in encoded) == (sys.argv[2] == "owned"), encoded
assert ("unrelated-hook" in encoded) == (sys.argv[3] == "unrelated"), encoded
PY
}

# Installed binary present, spaces in PREFIX/CODEX_HOME, unrelated hooks, and a
# repeated uninstall all remain safe and truthful.
PREFIX="$TEMP/prefix with spaces"
CODEX_HOME="$TEMP/codex home with spaces"
STATE="$TEMP/state with spaces"
mkdir -p "$CODEX_HOME"
printf '%s\n' '{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"unrelated-hook"}]}]}}' >"$CODEX_HOME/hooks.json"
CODEX_USAGE_WATCH_BINARY="$TEST_BIN" PREFIX="$PREFIX" CODEX_HOME="$CODEX_HOME" \
  CODEX_USAGE_WATCH_HOME="$STATE" INSTALL_HOOKS=1 "$ROOT/scripts/install.sh" >/dev/null
PREFIX="$PREFIX" CODEX_HOME="$CODEX_HOME" "$ROOT/scripts/uninstall.sh" --confirm >/dev/null
test ! -e "$PREFIX/bin/codex-watch"
assert_hook_state "$CODEX_HOME/hooks.json" absent unrelated
PREFIX="$PREFIX" CODEX_HOME="$CODEX_HOME" "$ROOT/scripts/uninstall.sh" --confirm >/dev/null
assert_hook_state "$CODEX_HOME/hooks.json" absent unrelated

# A missing binary with tracker hooks and no archive fallback stops without
# editing hooks or claiming success.
MISSING_PREFIX="$TEMP/missing-prefix"
MISSING_HOME="$TEMP/missing-home"
mkdir -p "$MISSING_PREFIX/bin" "$MISSING_HOME"
cp "$TEST_BIN" "$MISSING_PREFIX/bin/codex-watch"
PREFIX="$MISSING_PREFIX" CODEX_HOME="$MISSING_HOME" \
  "$MISSING_PREFIX/bin/codex-watch" install --confirm >/dev/null
rm "$MISSING_PREFIX/bin/codex-watch"
set +e
missing_output="$(PREFIX="$MISSING_PREFIX" CODEX_HOME="$MISSING_HOME" \
  "$ROOT/scripts/uninstall.sh" --confirm 2>&1)"
missing_status=$?
set -e
test "$missing_status" -eq 5
grep -F "Partial cleanup only" <<<"$missing_output" >/dev/null
grep -F "hook entries are still present" <<<"$missing_output" >/dev/null
assert_hook_state "$MISSING_HOME/hooks.json" owned absent

# The same missing-binary state is recoverable with the binary bundled in a
# checksum-verified release archive.
BUNDLE="$TEMP/bundle"
mkdir -p "$BUNDLE/scripts"
cp "$ROOT/scripts/uninstall.sh" "$BUNDLE/scripts/uninstall.sh"
cp "$TEST_BIN" "$BUNDLE/codex-watch"
printf '{}\n' >"$BUNDLE/BUILD-INFO.json"
printf '{}\n' >"$BUNDLE/SBOM.spdx.json"
PREFIX="$MISSING_PREFIX" CODEX_HOME="$MISSING_HOME" \
  "$BUNDLE/scripts/uninstall.sh" --confirm >/dev/null
assert_hook_state "$MISSING_HOME/hooks.json" absent absent

# With no installed binary and no owned hooks, cleanup is already complete;
# unrelated hooks are left byte-for-byte intact.
NO_HOOK_PREFIX="$TEMP/no-hook-prefix"
NO_HOOK_HOME="$TEMP/no-hook-home"
mkdir -p "$NO_HOOK_HOME"
printf '%s\n' '{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"unrelated-hook"}]}]}}' >"$NO_HOOK_HOME/hooks.json"
before="$(checksum "$NO_HOOK_HOME/hooks.json")"
PREFIX="$NO_HOOK_PREFIX" CODEX_HOME="$NO_HOOK_HOME" \
  "$ROOT/scripts/uninstall.sh" --confirm >/dev/null
after="$(checksum "$NO_HOOK_HOME/hooks.json")"
test "$before" = "$after"
assert_hook_state "$NO_HOOK_HOME/hooks.json" absent unrelated

echo "Uninstall present/missing/recovery/repeat/space-path scenarios: PASS"
