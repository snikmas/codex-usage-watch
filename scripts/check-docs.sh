#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

bash scripts/check-versions.sh
bash scripts/check-package-docs.sh "$ROOT"
bash scripts/test-acceptance-records.sh

python3 - "$ROOT" <<'PY'
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
public = [
    root / "README.md",
    root / "Cargo.toml",
    root / ".codex-plugin/plugin.json",
]
joined = "\n".join(path.read_text(encoding="utf-8") for path in public)

old_url = re.compile(r"https://github\.com/snikmas/codex-usage(?:[\s/#\"')]|$)")
if old_url.search(joined):
    raise SystemExit("public files contain the old codex-usage repository URL")

readme = (root / "README.md").read_text(encoding="utf-8")
for value in [
    "local estimate",
    "not official",
    "Four ways to see the output",
    "codex-watch status",
    "## Install",
    "## Privacy",
    "## Limitations",
    "## Contributing",
    "docs/ACCEPTANCE.md",
]:
    if value.lower() not in readme.lower():
        raise SystemExit(f"README is missing {value!r}")

if "codex-5h" in joined:
    raise SystemExit("public files still use the old codex-5h command")

print("README links, command name, and core sections: PASS")
PY
