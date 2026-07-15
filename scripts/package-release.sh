#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
TARGET="${CARGO_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
DIST="${DIST_DIR:-$ROOT/target/release-dist}"
NAME="codex-usage-watch-$VERSION-$TARGET"

cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked
cargo package --manifest-path "$ROOT/Cargo.toml" --locked --allow-dirty
mkdir -p "$DIST/$NAME"
install -m 0755 "$ROOT/target/release/codex-5h" "$DIST/$NAME/codex-5h"
install -m 0644 "$ROOT/README.md" "$ROOT/CHANGELOG.md" "$ROOT/LICENSE" \
  "$ROOT/SECURITY.md" "$ROOT/CONTRIBUTING.md" "$DIST/$NAME/"
install -d "$DIST/$NAME/docs"
install -m 0644 "$ROOT"/docs/*.md "$DIST/$NAME/docs/"
install -d "$DIST/$NAME/scripts"
install -m 0755 "$ROOT/scripts/install.sh" "$ROOT/scripts/uninstall.sh" \
  "$ROOT/scripts/verify-install.sh" "$DIST/$NAME/scripts/"
tar -C "$DIST" -czf "$DIST/$NAME.tar.gz" "$NAME"
cp "$ROOT/target/package/codex-usage-watch-$VERSION.crate" "$DIST/"
rm -rf "$DIST/$NAME"

cd "$DIST"
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$NAME.tar.gz" "codex-usage-watch-$VERSION.crate" >SHA256SUMS
else
  shasum -a 256 "$NAME.tar.gz" "codex-usage-watch-$VERSION.crate" >SHA256SUMS
fi
echo "Created versioned release artifacts and checksums in $DIST"
