#!/usr/bin/env bash
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin/codex-5h"

test -x "$BIN"
"$BIN" status >/dev/null
"$BIN" history --json >/dev/null
"$BIN" doctor >/dev/null
"$BIN" doctor --compat >/dev/null
python3 -m json.tool "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/.codex-plugin/plugin.json" >/dev/null
python3 -m json.tool "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/hooks/hooks.json" >/dev/null
echo "Verified $BIN, state migration, compatibility doctor, plugin manifest, and hook declarations"
