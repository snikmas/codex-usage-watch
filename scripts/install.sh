#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"

if [[ -n "${CODEX_USAGE_WATCH_BINARY:-}" ]]; then
  SOURCE_BIN="$CODEX_USAGE_WATCH_BINARY"
elif [[ -x "$ROOT/codex-5h" ]]; then
  SOURCE_BIN="$ROOT/codex-5h"
else
  cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked
  SOURCE_BIN="$ROOT/target/release/codex-5h"
fi
test -x "$SOURCE_BIN"
install -d "$BIN_DIR"
if [[ "$SOURCE_BIN" != "$BIN_DIR/codex-5h" ]]; then
  install -m 0755 "$SOURCE_BIN" "$BIN_DIR/codex-5h"
fi

if [[ "${INSTALL_HOOKS:-0}" == "1" ]]; then
  "$BIN_DIR/codex-5h" install --confirm
fi

INSTALLED_VERSION="$("$BIN_DIR/codex-5h" --version | awk '{print $2}')"
echo "Installed codex-5h ${CODEX_USAGE_WATCH_VERSION_SUFFIX:-tracker-$INSTALLED_VERSION} at $BIN_DIR/codex-5h"
echo "Next: $BIN_DIR/codex-5h setup --preview"
echo "Then: $BIN_DIR/codex-5h setup (history import is optional and consent-gated)"
