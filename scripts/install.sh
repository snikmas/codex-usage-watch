#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"

cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked
install -d "$BIN_DIR"
install -m 0755 "$ROOT/target/release/codex-5h" "$BIN_DIR/codex-5h"

if [[ "${INSTALL_HOOKS:-0}" == "1" ]]; then
  PATH="$BIN_DIR:$PATH" "$BIN_DIR/codex-5h" install --confirm
fi

echo "Installed codex-5h ${CODEX_USAGE_WATCH_VERSION_SUFFIX:-tracker-0.1.0} at $BIN_DIR/codex-5h"
echo "Next: $BIN_DIR/codex-5h setup --preview"
echo "Then: $BIN_DIR/codex-5h setup (history import is optional and consent-gated)"
