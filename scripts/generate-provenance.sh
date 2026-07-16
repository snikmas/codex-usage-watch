#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_DIR="${1:?usage: generate-provenance.sh OUTPUT_DIRECTORY}"
METADATA="$(mktemp)"
trap 'rm -f "$METADATA"' EXIT

mkdir -p "$OUTPUT_DIR"
cargo metadata --manifest-path "$ROOT/Cargo.toml" --locked --format-version 1 >"$METADATA"

SOURCE_REVISION="${GITHUB_SHA:-$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf unknown)}"
if [[ -n "$(git -C "$ROOT" status --porcelain 2>/dev/null || true)" ]]; then
  SOURCE_DIRTY=true
else
  SOURCE_DIRTY=false
fi
if [[ -n "${SOURCE_DATE_EPOCH:-}" ]]; then
  BUILD_EPOCH="$SOURCE_DATE_EPOCH"
else
  BUILD_EPOCH="$(git -C "$ROOT" show -s --format=%ct HEAD 2>/dev/null || printf 0)"
fi
TARGET="${CARGO_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"

python3 - "$ROOT" "$METADATA" "$OUTPUT_DIR" "$SOURCE_REVISION" \
  "$SOURCE_DIRTY" "$BUILD_EPOCH" "$TARGET" <<'PY'
import datetime
import hashlib
import json
import pathlib
import re
import subprocess
import sys

root = pathlib.Path(sys.argv[1])
metadata = json.loads(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8"))
output = pathlib.Path(sys.argv[3])
revision = sys.argv[4]
dirty = sys.argv[5].lower() == "true"
epoch = int(sys.argv[6])
target = sys.argv[7]
created = datetime.datetime.fromtimestamp(epoch, datetime.timezone.utc).strftime(
    "%Y-%m-%dT%H:%M:%SZ"
)
lock_hash = hashlib.sha256((root / "Cargo.lock").read_bytes()).hexdigest()
root_package = next(package for package in metadata["packages"] if package["name"] == "codex-usage-watch")


def spdx_id(package):
    identity = hashlib.sha256(package["id"].encode()).hexdigest()[:12]
    name = re.sub(r"[^A-Za-z0-9.-]", "-", package["name"])
    return f"SPDXRef-Package-{name}-{identity}"


packages = sorted(metadata["packages"], key=lambda package: package["id"])
ids = {package["id"]: spdx_id(package) for package in packages}
spdx_packages = []
for package in packages:
    source = package.get("source") or "NOASSERTION"
    spdx_packages.append(
        {
            "SPDXID": ids[package["id"]],
            "name": package["name"],
            "versionInfo": package["version"],
            "downloadLocation": source,
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": package.get("license") or "NOASSERTION",
            "copyrightText": "NOASSERTION",
        }
    )

relationships = [
    {
        "spdxElementId": "SPDXRef-DOCUMENT",
        "relationshipType": "DESCRIBES",
        "relatedSpdxElement": ids[root_package["id"]],
    }
]
nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
for package in packages:
    node = nodes.get(package["id"])
    if not node:
        continue
    for dependency in sorted(node["dependencies"]):
        relationships.append(
            {
                "spdxElementId": ids[package["id"]],
                "relationshipType": "DEPENDS_ON",
                "relatedSpdxElement": ids[dependency],
            }
        )

document = {
    "spdxVersion": "SPDX-2.3",
    "dataLicense": "CC0-1.0",
    "SPDXID": "SPDXRef-DOCUMENT",
    "name": f"codex-usage-watch-{root_package['version']}-{target}",
    "documentNamespace": (
        "https://github.com/snikmas/codex-usage-watch/"
        f"sbom/{root_package['version']}/{target}/{lock_hash}"
    ),
    "creationInfo": {
        "created": created,
        "creators": ["Tool: codex-usage-watch/scripts/generate-provenance.sh"],
    },
    "packages": spdx_packages,
    "relationships": relationships,
}
(output / "SBOM.spdx.json").write_text(
    json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)


def command(*arguments):
    return subprocess.check_output(arguments, text=True).strip()


build_info = {
    "product": "Codex Usage Watch",
    "version": root_package["version"],
    "target": target,
    "source_repository": "https://github.com/snikmas/codex-usage-watch",
    "source_revision": revision,
    "source_dirty": dirty,
    "source_timestamp_utc": created,
    "cargo_lock_sha256": lock_hash,
    "rustc": command("rustc", "--version"),
    "cargo": command("cargo", "--version"),
    "sbom": "SBOM.spdx.json",
}
(output / "BUILD-INFO.json").write_text(
    json.dumps(build_info, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)
PY

echo "Generated SPDX SBOM and build identity in $OUTPUT_DIR"
