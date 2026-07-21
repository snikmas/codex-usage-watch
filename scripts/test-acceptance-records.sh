#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VALID="$ROOT/tests/fixtures/acceptance_record_valid.json"

python3 "$ROOT/scripts/validate-acceptance-record.py" "$VALID" >/dev/null

TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT
python3 - "$VALID" "$TEMP/private.json" "$TEMP/error.json" <<'PY'
import json
import pathlib
import sys

source = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
json.loads((pathlib.Path(sys.argv[1]).parents[2] / "docs" / "acceptance-record-v1.schema.json").read_text(encoding="utf-8"))
private = dict(source)
private["transcript_path"] = "/private/session.jsonl"
pathlib.Path(sys.argv[2]).write_text(json.dumps(private), encoding="utf-8")

error = json.loads(json.dumps(source))
error["ground_truth"]["absolute_error_points"] = 99
pathlib.Path(sys.argv[3]).write_text(json.dumps(error), encoding="utf-8")
PY

if python3 "$ROOT/scripts/validate-acceptance-record.py" "$TEMP/private.json" >/dev/null 2>&1; then
  echo "validator accepted a private field" >&2
  exit 1
fi
if python3 "$ROOT/scripts/validate-acceptance-record.py" "$TEMP/error.json" >/dev/null 2>&1; then
  echo "validator accepted an incorrect absolute error" >&2
  exit 1
fi

echo "Acceptance record schema, privacy allowlist, and error calculation: PASS"
