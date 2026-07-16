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
    "Ubuntu 25.10 x86_64",
    "Codex CLI 0.144.4",
    "experimental beta",
    "macOS",
    "Windows",
    "0.1.0-beta.1",
]:
    if value not in joined:
        raise SystemExit(f"public truth is missing {value!r}")

support = (root / "docs/SUPPORT.md").read_text(encoding="utf-8")
for statement in [
    "Ubuntu 25.10 x86_64 standalone archive | Experimental beta candidate",
    "Other Linux distributions or architectures | Unverified",
    "macOS library and CLI | Preview",
    "Windows library and CLI | Build/test only",
    "Native footer and `/status` adapter | Development preview",
]:
    if statement not in support:
        raise SystemExit(f"support matrix changed or contradicted: {statement}")

for path in [
    root / "README.md",
    root / "docs/INSTALL.md",
    root / "docs/SUPPORT.md",
    root / "docs/RELEASE.md",
    root / "docs/RELEASE_NOTES.md",
]:
    text = path.read_text(encoding="utf-8")
    for unsupported in ["Linux x86_64", "all Linux", "general Linux support"]:
        if unsupported.lower() in text.lower():
            raise SystemExit(f"{path.relative_to(root)} contains unsupported platform claim {unsupported!r}")

first_screen = " ".join((root / "README.md").read_text(encoding="utf-8").splitlines()[:35])
first_screen = " ".join(first_screen.split())
for value in [
    "Experimental beta",
    "Ubuntu 25.10 x86_64",
    "local estimate",
    "not official",
    "private vulnerability reporting",
]:
    if value.lower() not in first_screen.lower():
        raise SystemExit(f"README first screen is missing {value!r}")

print("Repository URLs, versions, links, and support claims: PASS")
PY
