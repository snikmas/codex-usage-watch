#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: scripts/verify-artifact-behavior.sh RELEASE_ARCHIVE.tar.gz" >&2
  exit 2
fi

ARCHIVE="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT
tar -xzf "$ARCHIVE" -C "$TEMP"
PACKAGE_ROOT="$(find "$TEMP" -mindepth 1 -maxdepth 1 -type d -name 'codex-usage-watch-*' -print -quit)"
BIN="$PACKAGE_ROOT/codex-watch"
test -x "$BIN"

write_case() {
  python3 - "$1" "$2" <<'PY'
import datetime
import json
import sys

path, case = sys.argv[1:]
now = datetime.datetime.now(datetime.timezone.utc)

def line(at, used, reset):
    return {
        "timestamp": at.isoformat().replace("+00:00", "Z"),
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "rate_limits": {
                "plan_type": "plus",
                "primary": {
                    "used_percent": used,
                    "window_minutes": 10080,
                    "resets_at": int(reset.timestamp()),
                },
            },
        },
    }

reset = now + datetime.timedelta(days=7)
if case == "super":
    records = [line(now - datetime.timedelta(minutes=2), 20, reset), line(now - datetime.timedelta(minutes=1), 38, reset)]
elif case == "stale":
    records = [line(now - datetime.timedelta(minutes=21), 20, reset), line(now - datetime.timedelta(minutes=20), 22, reset)]
elif case == "reset":
    old = now + datetime.timedelta(days=1)
    new = now + datetime.timedelta(days=8)
    records = [
        line(now - datetime.timedelta(minutes=4), 98, old),
        line(now - datetime.timedelta(minutes=3), 99, old),
        line(now - datetime.timedelta(minutes=2), 1, new),
        line(now - datetime.timedelta(minutes=1), 3, new),
    ]
elif case == "concurrent-a":
    records = [line(now - datetime.timedelta(minutes=4), 20, reset), line(now - datetime.timedelta(minutes=3), 21, reset)]
elif case == "concurrent-b":
    records = [line(now - datetime.timedelta(minutes=2), 21, reset), line(now - datetime.timedelta(minutes=1), 22, reset)]
else:
    raise SystemExit("unknown case")

with open(path, "w", encoding="utf-8") as handle:
    for record in records:
        handle.write(json.dumps(record, separators=(",", ":")) + "\n")
PY
}

export CODEX_HOME="$TEMP/codex-home"
mkdir -p "$CODEX_HOME"

export CODEX_USAGE_WATCH_HOME="$TEMP/missing-state"
"$BIN" status --json | python3 -c 'import json,sys; assert json.load(sys.stdin)["display"]["status"] == "unknown"'

write_case "$TEMP/super.jsonl" super
export CODEX_USAGE_WATCH_HOME="$TEMP/super-state"
"$BIN" refresh --transcript "$TEMP/super.jsonl" >/dev/null
"$BIN" status --json | python3 -c 'import json,sys; d=json.load(sys.stdin)["display"]; assert d["five_hour_estimate_percent"] > 100'

write_case "$TEMP/stale.jsonl" stale
export CODEX_USAGE_WATCH_HOME="$TEMP/stale-state"
"$BIN" refresh --transcript "$TEMP/stale.jsonl" >/dev/null
"$BIN" status --json | python3 -c 'import json,sys; assert json.load(sys.stdin)["display"]["status"] == "stale"'

write_case "$TEMP/reset.jsonl" reset
export CODEX_USAGE_WATCH_HOME="$TEMP/reset-state"
"$BIN" refresh --transcript "$TEMP/reset.jsonl" >/dev/null
"$BIN" status --json | python3 -c 'import json,sys; d=json.load(sys.stdin)["display"]; assert 24 <= d["five_hour_estimate_percent"] <= 26'

write_case "$TEMP/a.jsonl" concurrent-a
write_case "$TEMP/b.jsonl" concurrent-b
export CODEX_USAGE_WATCH_HOME="$TEMP/concurrent-state"
"$BIN" refresh --transcript "$TEMP/a.jsonl" >/dev/null & A=$!
"$BIN" refresh --transcript "$TEMP/b.jsonl" >/dev/null & B=$!
wait "$A"
wait "$B"
"$BIN" status --json | python3 -c 'import json,sys; d=json.load(sys.stdin)["display"]; assert 12 <= d["five_hour_estimate_percent"] <= 13'

"$BIN" --help | grep -F "Estimates are not official quota or billing data" >/dev/null
echo "Exact artifact missing, stale, reset, concurrent, above-100%, and wording checks: PASS"
