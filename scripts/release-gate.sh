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
ARCHIVE="$(find target/release-dist -maxdepth 1 -name '*.tar.gz' -print -quit)"
bash scripts/verify-artifact-behavior.sh "$ARCHIVE"

if find target/release-dist -type f -print0 | xargs -0 grep -aE -n \
  '/home/snikmas|notes/agent|\.agent/|rollout-[0-9]|transcript_id' >/dev/null; then
  echo "release artifact privacy scan found a forbidden local/private marker" >&2
  exit 2
fi

echo "Local exact-artifact release gate: PASS"
