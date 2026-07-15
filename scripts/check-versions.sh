#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
PLUGIN_VERSION="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["version"])' "$ROOT/.codex-plugin/plugin.json")"
CHANGELOG_VERSION="$(sed -n 's/^## \([^ ]*\) .*/\1/p' "$ROOT/CHANGELOG.md" | head -n 1)"

[[ -n "$CARGO_VERSION" ]]
[[ "$CARGO_VERSION" == "$PLUGIN_VERSION" ]]
[[ "$CARGO_VERSION" == "$CHANGELOG_VERSION" ]]
for document in "$ROOT/README.md" "$ROOT/docs/INSTALL.md" "$ROOT/docs/RELEASE.md" "$ROOT/docs/RELEASE_NOTES.md" "$ROOT/docs/SUPPORT.md"; do
  grep -F "$CARGO_VERSION" "$document" >/dev/null
done
echo "Version surfaces agree: $CARGO_VERSION"
