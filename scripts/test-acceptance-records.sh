#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VALID="$ROOT/tests/fixtures/acceptance_record_valid.json"

python3 "$ROOT/scripts/validate-acceptance-record.py" "$VALID" >/dev/null

TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT
python3 - "$VALID" "$TEMP" <<'PY'
import json
import pathlib
import sys

source = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
json.loads((pathlib.Path(sys.argv[1]).parents[2] / "docs" / "acceptance-record-v1.schema.json").read_text(encoding="utf-8"))
directory = pathlib.Path(sys.argv[2])
mutations = {
    "private": lambda value: value.update(transcript_path="/private/session.jsonl"),
    "error": lambda value: value["ground_truth"].update(absolute_error_points=99),
    "date-only": lambda value: value.update(recorded_at="2030-01-01"),
    "timezone-less": lambda value: value["window"].update(observed_at="2030-01-01T16:59:00"),
    "reversed": lambda value: value["window"].update(observed_at="2030-01-01T11:59:00Z"),
    "free-form": lambda value: value["environment"].update(codex_version="private user text"),
}
for name, mutate in mutations.items():
    invalid = json.loads(json.dumps(source))
    mutate(invalid)
    (directory / f"{name}.json").write_text(json.dumps(invalid), encoding="utf-8")
PY

for invalid in private error date-only timezone-less reversed free-form; do
  if python3 "$ROOT/scripts/validate-acceptance-record.py" "$TEMP/$invalid.json" >/dev/null 2>&1; then
    echo "validator accepted invalid fixture: $invalid" >&2
    exit 1
  fi
done

echo "Acceptance record chronology, privacy allowlist, and error calculation: PASS"
