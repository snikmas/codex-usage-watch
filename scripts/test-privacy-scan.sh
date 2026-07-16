#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
TARGET="${CARGO_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
DIST="${DIST_DIR:-$ROOT/target/release-dist}"
ARCHIVE="$DIST/codex-usage-watch-$VERSION-$TARGET.tar.gz"
CRATE="$DIST/codex-usage-watch-$VERSION.crate"

bash "$ROOT/scripts/scan-release-privacy.sh" "$ARCHIVE" "$CRATE"

TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT
CONTAMINATED="$TEMP/contaminated.tar.gz"
python3 - "$ARCHIVE" "$CONTAMINATED" <<'PY'
import io
import sys
import tarfile

source, destination = sys.argv[1:]
with tarfile.open(source, "r:gz") as original, tarfile.open(destination, "w:gz") as output:
    for member in original.getmembers():
        output.addfile(member, original.extractfile(member) if member.isfile() else None)
    payload = b"private release contamination regression"
    member = tarfile.TarInfo("codex-usage-watch-contaminated/notes/private.txt")
    member.size = len(payload)
    output.addfile(member, io.BytesIO(payload))
PY

if bash "$ROOT/scripts/scan-release-privacy.sh" "$CONTAMINATED" "$CRATE" >/dev/null 2>&1; then
  echo "privacy scan accepted a deliberately contaminated archive" >&2
  exit 2
fi
echo "Contaminated archive rejection: PASS"
