#!/usr/bin/env bash
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin/codex-5h"

test -x "$BIN"
"$BIN" --help >/dev/null
"$BIN" --version >/dev/null
"$BIN" status >/dev/null
"$BIN" status --json | python3 -m json.tool >/dev/null
"$BIN" history --json >/dev/null
"$BIN" doctor >/dev/null
"$BIN" doctor --compat >/dev/null
echo "Verified $BIN, CLI contracts, state migration, absolute hook commands, and compatibility doctor"
