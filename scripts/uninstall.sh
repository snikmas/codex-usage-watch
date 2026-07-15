#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != "--confirm" ]]; then
  echo "Refusing to uninstall without --confirm" >&2
  exit 2
fi

PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin/codex-5h"
if [[ -x "$BIN" ]]; then
  "$BIN" uninstall --confirm
  rm -f "$BIN"
fi

echo "Removed the codex-5h binary and its hook entries."
echo "Tracker state was preserved. Remove it manually only after making a backup."
