#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"

if [[ -n "${CODEX_USAGE_WATCH_BINARY:-}" ]]; then
  SOURCE_BIN="$CODEX_USAGE_WATCH_BINARY"
elif [[ -x "$ROOT/codex-watch" ]]; then
  SOURCE_BIN="$ROOT/codex-watch"
else
  cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked
  SOURCE_BIN="$ROOT/target/release/codex-watch"
fi
test -x "$SOURCE_BIN"
install -d "$BIN_DIR"
if [[ "$SOURCE_BIN" != "$BIN_DIR/codex-watch" ]]; then
  install -m 0755 "$SOURCE_BIN" "$BIN_DIR/codex-watch"
fi

if [[ "${INSTALL_HOOKS:-0}" == "1" ]]; then
  "$BIN_DIR/codex-watch" install --confirm
  echo "Required: start or restart Codex, open /hooks, inspect the source and all three commands, trust them, then start a fresh session."
fi

INSTALLED_VERSION="$("$BIN_DIR/codex-watch" --version | awk '{print $2}')"
echo "Installed codex-watch ${CODEX_USAGE_WATCH_VERSION_SUFFIX:-tracker-$INSTALLED_VERSION} at $BIN_DIR/codex-watch"
echo "Next: $BIN_DIR/codex-watch setup --preview"
echo "Then: $BIN_DIR/codex-watch setup (history import is optional and consent-gated)"
