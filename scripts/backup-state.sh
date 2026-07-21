#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: scripts/backup-state.sh DESTINATION.sqlite3" >&2
  exit 2
fi

PREFIX="${PREFIX:-$HOME/.local}"
"$PREFIX/bin/codex-watch" backup "$1" --confirm
python3 - "$1" <<'PY'
import sqlite3
import sys

connection = sqlite3.connect(sys.argv[1])
result = connection.execute("PRAGMA integrity_check").fetchone()[0]
if result != "ok":
    raise SystemExit(f"backup integrity check failed: {result}")
print("Backup integrity: ok")
PY
