#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
TARGET="${CARGO_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
DIST="${DIST_DIR:-$ROOT/target/release-dist}"
NAME="codex-usage-watch-$VERSION-$TARGET"
BUILD_HOME="$(python3 -c 'import pathlib; print(pathlib.Path.home())')"
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$ROOT=/src/codex-usage-watch --remap-path-prefix=$BUILD_HOME=/build-home"

cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked
cargo package --manifest-path "$ROOT/Cargo.toml" --locked --allow-dirty
rm -rf "$DIST/$NAME"
mkdir -p "$DIST/$NAME"
install -m 0755 "$ROOT/target/release/codex-watch" "$DIST/$NAME/codex-watch"
install -m 0644 "$ROOT/README.md" "$ROOT/LICENSE" "$DIST/$NAME/"
install -d "$DIST/$NAME/docs/images"
install -m 0644 "$ROOT"/docs/images/*.png "$DIST/$NAME/docs/images/"
install -d "$DIST/$NAME/scripts"
install -m 0755 "$ROOT/scripts/install.sh" "$ROOT/scripts/uninstall.sh" \
  "$ROOT/scripts/verify-install.sh" "$ROOT/scripts/backup-state.sh" \
  "$ROOT/scripts/check-package-docs.sh" "$DIST/$NAME/scripts/"
bash "$ROOT/scripts/generate-provenance.sh" "$DIST/$NAME"
tar -C "$DIST" -czf "$DIST/$NAME.tar.gz" "$NAME"
cp "$ROOT/target/package/codex-usage-watch-$VERSION.crate" "$DIST/"
rm -rf "$DIST/$NAME"

cd "$DIST"
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$NAME.tar.gz" >SHA256SUMS
else
  shasum -a 256 "$NAME.tar.gz" >SHA256SUMS
fi

for required in \
  "$NAME/codex-watch" \
  "$NAME/README.md" \
  "$NAME/BUILD-INFO.json" \
  "$NAME/SBOM.spdx.json" \
  "$NAME/docs/images/terminal-status.png" \
  "$NAME/scripts/install.sh" \
  "$NAME/scripts/verify-install.sh" \
  "$NAME/scripts/backup-state.sh" \
  "$NAME/scripts/uninstall.sh" \
  "$NAME/scripts/check-package-docs.sh"; do
  tar -tzf "$NAME.tar.gz" | grep -Fx "$required" >/dev/null || {
    echo "release archive is missing $required" >&2
    exit 2
  }
done
echo "Created versioned release artifacts and checksums in $DIST"
