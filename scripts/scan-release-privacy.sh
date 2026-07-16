#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARCHIVE="${1:?usage: scan-release-privacy.sh ARCHIVE CRATE}"
CRATE="${2:?usage: scan-release-privacy.sh ARCHIVE CRATE}"

python3 - "$ROOT" "$ARCHIVE" "$CRATE" <<'PY'
import io
import pathlib
import shutil
import sys
import tarfile
import tempfile

root = pathlib.Path(sys.argv[1]).resolve()
archive = pathlib.Path(sys.argv[2]).resolve()
crate = pathlib.Path(sys.argv[3]).resolve()

for artifact in (archive, crate):
    if not artifact.is_file():
        raise SystemExit(f"missing artifact: {artifact}")

forbidden_content = [
    b"/home/" + b"snikmas",
    b"rollout" + b"-20",
    b"transcript" + b"_id",
]
forbidden_parts = {".agent", ".agents", "notes"}

def safe_extract(source: pathlib.Path, destination: pathlib.Path) -> list[str]:
    with tarfile.open(source, "r:gz") as bundle:
        names = []
        for member in bundle.getmembers():
            normalized = pathlib.PurePosixPath(member.name)
            if normalized.is_absolute() or ".." in normalized.parts:
                raise SystemExit(f"unsafe archive path: {member.name}")
            names.append(member.name.rstrip("/"))
        bundle.extractall(destination, filter="data")
        return names

def scan_tree(tree: pathlib.Path, *, crate_tree: bool) -> None:
    for path in tree.rglob("*"):
        relative = path.relative_to(tree)
        if any(part in forbidden_parts for part in relative.parts):
            raise SystemExit(f"forbidden private path in artifact: {relative}")
        if path.is_dir():
            continue
        if path.suffix == ".jsonl":
            allowed_fixture = crate_tree and "tests" in relative.parts and "fixtures" in relative.parts
            if not allowed_fixture:
                raise SystemExit(f"unexpected JSONL file in artifact: {relative}")
        if path.name in {"state.sqlite3", "state.sqlite3-wal", "state.sqlite3-shm"}:
            raise SystemExit(f"private state database in artifact: {relative}")
        data = path.read_bytes()
        for marker in forbidden_content:
            if marker in data:
                raise SystemExit(f"forbidden local/private marker in artifact: {relative}")

with tempfile.TemporaryDirectory(prefix="codex-usage-watch-privacy-") as temporary:
    temporary = pathlib.Path(temporary)
    archive_tree = temporary / "archive"
    crate_tree = temporary / "crate"
    archive_tree.mkdir()
    crate_tree.mkdir()
    archive_names = safe_extract(archive, archive_tree)
    safe_extract(crate, crate_tree)
    scan_tree(archive_tree, crate_tree=False)
    scan_tree(crate_tree, crate_tree=True)

    package_roots = [path for path in archive_tree.iterdir() if path.is_dir()]
    if len(package_roots) != 1:
        raise SystemExit("release archive must contain exactly one package directory")
    package_root = package_roots[0]
    expected_files = {
        "BUILD-INFO.json",
        "CHANGELOG.md",
        "CONTRIBUTING.md",
        "LICENSE",
        "README.md",
        "SBOM.spdx.json",
        "SECURITY.md",
        "codex-5h",
        *{f"docs/{path.name}" for path in (root / "docs").glob("*.md")},
        "scripts/backup-state.sh",
        "scripts/check-package-docs.sh",
        "scripts/install.sh",
        "scripts/uninstall.sh",
        "scripts/verify-install.sh",
    }
    actual_files = {
        str(path.relative_to(package_root))
        for path in package_root.rglob("*")
        if path.is_file()
    }
    missing = sorted(expected_files - actual_files)
    unexpected = sorted(actual_files - expected_files)
    if missing or unexpected:
        raise SystemExit(f"archive manifest mismatch; missing={missing}, unexpected={unexpected}")

print("Extracted archive/crate privacy and manifest scan: PASS")
PY
