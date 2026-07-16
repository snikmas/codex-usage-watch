#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python3 - "$ROOT/.codex-plugin/plugin.json" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as source:
    plugin = json.load(source)
for key in ("name", "version", "description"):
    if not isinstance(plugin.get(key), str) or not plugin[key].strip():
        raise SystemExit(f"plugin manifest requires non-empty {key}")
if not isinstance(plugin.get("author"), dict) or not plugin["author"].get("name"):
    raise SystemExit("plugin manifest requires structured author.name")
interface = plugin.get("interface")
if not isinstance(interface, dict) or not interface.get("defaultPrompt"):
    raise SystemExit("plugin manifest requires interface.defaultPrompt")
if "hooks" in plugin:
    raise SystemExit("plugin manifest must not declare unsupported top-level hooks")
print("Plugin manifest structure: PASS")
PY
