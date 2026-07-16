#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ "${ALLOW_DIRTY:-0}" != "1" ]] && [[ -n "$(git status --porcelain)" ]]; then
  echo "release gate requires a clean worktree" >&2
  exit 2
fi

bash scripts/check-versions.sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
bash scripts/smoke-install.sh
bash scripts/package-release.sh

(cd target/release-dist && sha256sum -c SHA256SUMS)
VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
TARGET="${CARGO_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
ARCHIVE="target/release-dist/codex-usage-watch-$VERSION-$TARGET.tar.gz"
test -f "$ARCHIVE"
bash scripts/verify-artifact-behavior.sh "$ARCHIVE"

# Reproduce the public INSTALL.md path from a blank directory. From this point
# through uninstall, every executable and helper comes from the release archive.
PUBLIC_TEMP="$(mktemp -d)"
trap 'rm -rf "$PUBLIC_TEMP"' EXIT
cp "$ARCHIVE" target/release-dist/SHA256SUMS "$PUBLIC_TEMP/"
(
  cd "$PUBLIC_TEMP"
  sha256sum -c SHA256SUMS
  tar -xzf "$(basename "$ARCHIVE")"
)
PACKAGE_ROOT="$(find "$PUBLIC_TEMP" -mindepth 1 -maxdepth 1 -type d -name 'codex-usage-watch-*' -print -quit)"
test -n "$PACKAGE_ROOT"
bash "$PACKAGE_ROOT/scripts/check-package-docs.sh" "$PACKAGE_ROOT"
(cd "$PACKAGE_ROOT" && python3 -m json.tool BUILD-INFO.json >/dev/null)
(cd "$PACKAGE_ROOT" && python3 -m json.tool SBOM.spdx.json >/dev/null)
python3 - "$PACKAGE_ROOT" <<'PY'
import json
import os
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
build = json.loads((root / "BUILD-INFO.json").read_text(encoding="utf-8"))
sbom = json.loads((root / "SBOM.spdx.json").read_text(encoding="utf-8"))
assert build["version"] == "0.1.0-beta.1"
if os.environ.get("ALLOW_DIRTY") != "1":
    assert build["source_dirty"] is False
assert build["sbom"] == "SBOM.spdx.json"
assert sbom["spdxVersion"] == "SPDX-2.3"
assert any(package["name"] == "codex-usage-watch" for package in sbom["packages"])
assert any(package["name"] == "rusqlite" for package in sbom["packages"])
PY

export PREFIX="$PUBLIC_TEMP/prefix"
export CODEX_HOME="$PUBLIC_TEMP/codex-home"
export CODEX_USAGE_WATCH_HOME="$PUBLIC_TEMP/state"
mkdir -p "$CODEX_HOME"
printf '%s\n' '{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"unrelated-hook"}]}]}}' >"$CODEX_HOME/hooks.json"
INSTALL_HOOKS=1 bash "$PACKAGE_ROOT/scripts/install.sh"
"$PREFIX/bin/codex-5h" setup --skip-import >/dev/null
bash "$PACKAGE_ROOT/scripts/verify-install.sh"
bash "$PACKAGE_ROOT/scripts/backup-state.sh" "$PUBLIC_TEMP/backup.sqlite3"

# Restore from the integrity-checked backup using only archive-provided tools.
"$PREFIX/bin/codex-5h" uninstall --confirm >/dev/null
rm -f "$CODEX_USAGE_WATCH_HOME/state.sqlite3-wal" "$CODEX_USAGE_WATCH_HOME/state.sqlite3-shm"
cp "$PUBLIC_TEMP/backup.sqlite3" "$CODEX_USAGE_WATCH_HOME/state.sqlite3"
INSTALL_HOOKS=1 bash "$PACKAGE_ROOT/scripts/install.sh"
bash "$PACKAGE_ROOT/scripts/verify-install.sh"

# Upgrade and rollback both preserve state. The rollback binary is a copy of the
# verified archive binary, never a source-checkout build.
cp "$PREFIX/bin/codex-5h" "$PUBLIC_TEMP/prior-codex-5h"
INSTALL_HOOKS=1 bash "$PACKAGE_ROOT/scripts/install.sh"
"$PREFIX/bin/codex-5h" uninstall --confirm >/dev/null
install -m 0755 "$PUBLIC_TEMP/prior-codex-5h" "$PREFIX/bin/codex-5h"
"$PREFIX/bin/codex-5h" install --confirm >/dev/null
bash "$PACKAGE_ROOT/scripts/verify-install.sh"
bash "$PACKAGE_ROOT/scripts/uninstall.sh" --confirm
test -f "$CODEX_USAGE_WATCH_HOME/state.sqlite3"
python3 - "$CODEX_HOME/hooks.json" <<'PY'
import json
import sys

encoded = json.dumps(json.load(open(sys.argv[1], encoding="utf-8")))
assert "unrelated-hook" in encoded
assert "codex-5h hook" not in encoded
PY

if find target/release-dist -type f -print0 | xargs -0 grep -aE -n \
  '/home/snikmas|notes/agent|\.agent/|rollout-[0-9]|transcript_id' >/dev/null; then
  echo "release artifact privacy scan found a forbidden local/private marker" >&2
  exit 2
fi

echo "Local exact-artifact release gate: PASS"
