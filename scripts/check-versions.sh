#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
PLUGIN_VERSION="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["version"])' "$ROOT/.codex-plugin/plugin.json")"
[[ -n "$CARGO_VERSION" ]]
[[ "$CARGO_VERSION" == "$PLUGIN_VERSION" ]]
echo "Version surfaces agree: $CARGO_VERSION"
