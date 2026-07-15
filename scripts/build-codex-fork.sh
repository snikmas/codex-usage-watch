#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: scripts/build-codex-fork.sh --source CODEX_CHECKOUT --ref GIT_REF --suffix SUFFIX --output DIR" >&2
  exit 2
}

SOURCE=""
REF=""
SUFFIX=""
OUTPUT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --source) SOURCE="${2:-}"; shift 2 ;;
    --ref) REF="${2:-}"; shift 2 ;;
    --suffix) SUFFIX="${2:-}"; shift 2 ;;
    --output) OUTPUT="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$SOURCE" && -n "$REF" && -n "$SUFFIX" && -n "$OUTPUT" ]] || usage

ACTUAL_REF="$(git -C "$SOURCE" rev-parse HEAD)"
EXPECTED_REF="$(git -C "$SOURCE" rev-parse "$REF^{commit}")"
[[ "$ACTUAL_REF" == "$EXPECTED_REF" ]] || {
  echo "Codex checkout must already be at requested ref $REF; this script will not mutate it" >&2
  exit 2
}

test -d "$SOURCE/codex-rs/tui"
(cd "$SOURCE" && just test -p codex-tui)
(cd "$SOURCE/codex-rs" && cargo build --locked --release -p codex-cli)
mkdir -p "$OUTPUT"
install -m 0755 "$SOURCE/codex-rs/target/release/codex" "$OUTPUT/codex-$SUFFIX"
SHA256="$(sha256sum "$OUTPUT/codex-$SUFFIX" | awk '{print $1}')"
cat >"$OUTPUT/codex-$SUFFIX.build-info" <<EOF
fork_release_suffix=$SUFFIX
upstream_ref=$ACTUAL_REF
tracker_release=0.1.0
binary_sha256=$SHA256
focused_test=just test -p codex-tui
EOF
echo "Built identifiable fork artifact $OUTPUT/codex-$SUFFIX at $ACTUAL_REF"
