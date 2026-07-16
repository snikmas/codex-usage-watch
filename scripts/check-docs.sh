#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

bash scripts/check-versions.sh
bash scripts/check-package-docs.sh "$ROOT"

python3 - "$ROOT" <<'PY'
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
public = [
    root / "README.md",
    root / "CHANGELOG.md",
    root / "CONTRIBUTING.md",
    root / "SECURITY.md",
    root / "Cargo.toml",
    root / ".codex-plugin/plugin.json",
    *sorted((root / "docs").glob("*.md")),
]
joined = "\n".join(path.read_text(encoding="utf-8") for path in public)

old_url = re.compile(r"https://github\.com/snikmas/codex-usage(?:[\s/#\"')]|$)")
if old_url.search(joined):
    raise SystemExit("public files contain the old codex-usage repository URL")

for value in [
    "https://github.com/snikmas/codex-usage-watch",
    "Linux x86_64",
    "macOS",
    "Windows",
    "0.1.0-beta.1",
]:
    if value not in joined:
        raise SystemExit(f"public truth is missing {value!r}")

support = (root / "docs/SUPPORT.md").read_text(encoding="utf-8")
for statement in [
    "Linux x86_64 standalone archive | Beta candidate",
    "macOS Rust library and CLI | CI-configured preview",
    "Windows Rust library and CLI | CI-configured build-only",
    "Native Codex footer and `/status` adapter | Development preview",
]:
    if statement not in support:
        raise SystemExit(f"support matrix changed or contradicted: {statement}")

print("Repository URLs, versions, links, and support claims: PASS")
PY
