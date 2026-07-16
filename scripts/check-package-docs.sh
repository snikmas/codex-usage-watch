#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

python3 - "$ROOT" <<'PY'
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1]).resolve()
markdown = [root / "README.md", *sorted((root / "docs").glob("*.md"))]
missing = []

for document in markdown:
    if not document.is_file():
        missing.append(f"missing packaged document: {document.relative_to(root)}")
        continue
    text = document.read_text(encoding="utf-8")
    for target in re.findall(r"\[[^\]]+\]\(([^)]+)\)", text):
        target = target.strip().split("#", 1)[0]
        if not target or "://" in target or target.startswith(("mailto:", "/")):
            continue
        resolved = (document.parent / target).resolve()
        try:
            resolved.relative_to(root)
        except ValueError:
            missing.append(f"{document.relative_to(root)} escapes package root: {target}")
            continue
        if not resolved.exists():
            missing.append(f"{document.relative_to(root)} links to missing {target}")
    for script in sorted(set(re.findall(r"scripts/[A-Za-z0-9._-]+\.sh", text))):
        path = root / script
        if not path.is_file():
            missing.append(f"{document.relative_to(root)} references missing {script}")

if missing:
    raise SystemExit("\n".join(missing))
print("Packaged Markdown links and script references: PASS")
PY
