#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VALID="$ROOT/tests/fixtures/acceptance_record_valid.json"
STAGE15_VALID="$ROOT/docs/acceptance-record-stage15.example.json"

python3 "$ROOT/scripts/validate-acceptance-record.py" "$VALID" >/dev/null
python3 "$ROOT/scripts/validate-acceptance-record.py" \
  --require-stage 15 "$STAGE15_VALID" >/dev/null

TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT
python3 - "$VALID" "$STAGE15_VALID" "$TEMP" <<'PY'
import json
import pathlib
import sys

source = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
json.loads((pathlib.Path(sys.argv[1]).parents[2] / "docs" / "acceptance-record-v1.schema.json").read_text(encoding="utf-8"))
stage15_source = json.loads(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8"))
directory = pathlib.Path(sys.argv[3])
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

legacy_stage14 = json.loads(json.dumps(source))
legacy_stage14.pop("lifecycle")
legacy_stage14["environment"].pop("artifact_source")
(directory / "legacy-stage14.json").write_text(
    json.dumps(legacy_stage14),
    encoding="utf-8",
)

stage15_mutations = {
    "stage15-maintainer": lambda value: value.update(observer_role="maintainer"),
    "stage15-local-artifact": lambda value: value["environment"].update(artifact_source="local_candidate"),
    "stage15-no-trust": lambda value: value["lifecycle"].update(hook_trust="not_observed"),
    "stage15-missing-hook": lambda value: value["lifecycle"].update(real_hooks=["SessionStart", "Stop"]),
    "stage15-private-help": lambda value: value["usability"].update(note_codes=["needed_unpublished_help"]),
}
for name, mutate in stage15_mutations.items():
    invalid = json.loads(json.dumps(stage15_source))
    mutate(invalid)
    (directory / f"{name}.json").write_text(json.dumps(invalid), encoding="utf-8")
PY

python3 "$ROOT/scripts/validate-acceptance-record.py" \
  "$TEMP/legacy-stage14.json" >/dev/null

for invalid in private error date-only timezone-less reversed free-form; do
  if python3 "$ROOT/scripts/validate-acceptance-record.py" "$TEMP/$invalid.json" >/dev/null 2>&1; then
    echo "validator accepted invalid fixture: $invalid" >&2
    exit 1
  fi
done

for invalid in stage15-maintainer stage15-local-artifact stage15-no-trust stage15-missing-hook stage15-private-help; do
  if python3 "$ROOT/scripts/validate-acceptance-record.py" \
    --require-stage 15 "$TEMP/$invalid.json" >/dev/null 2>&1; then
    echo "Stage 15 validator accepted invalid fixture: $invalid" >&2
    exit 1
  fi
done

echo "Acceptance chronology, privacy, calculation, and Stage 15 lifecycle: PASS"
