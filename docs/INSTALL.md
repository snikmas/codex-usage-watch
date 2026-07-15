# Standalone beta installation

The only supported beta distribution is the checksummed standalone Linux
x86_64 archive. It contains `codex-5h`, documentation, and explicit install,
verification, and uninstall scripts. It does not replace the official Codex
binary and does not include the native footer adapter.

## Install and verify

Download the archive and `SHA256SUMS` from the same beta release, then verify the
archive before extracting it:

```bash
sha256sum -c SHA256SUMS
tar -xzf codex-usage-watch-0.1.0-beta.1-x86_64-unknown-linux-gnu.tar.gz
cd codex-usage-watch-0.1.0-beta.1-x86_64-unknown-linux-gnu
PREFIX="$HOME/.local" INSTALL_HOOKS=1 scripts/install.sh
PREFIX="$HOME/.local" scripts/verify-install.sh
```

Read the generated `$CODEX_HOME/hooks.json` entry before starting a new Codex
session. Each of the three handlers must call the absolute installed
`codex-5h` path. Installation saves a recoverable
`hooks.json.codex-5h.bak` before changing an existing hooks file.

Initialize without importing history:

```bash
"$HOME/.local/bin/codex-5h" setup --skip-import
"$HOME/.local/bin/codex-5h" status
"$HOME/.local/bin/codex-5h" doctor
```

History import is optional and consent-gated; use `setup --preview` first.

## Upgrade, disable, and uninstall

To upgrade, verify the new checksum, extract the new archive, and rerun its
installer and verifier. The SQLite state is migrated forward and retained.

To temporarily disable the tracker, remove only its hooks:

```bash
"$HOME/.local/bin/codex-5h" uninstall --confirm
```

To remove both hooks and the installed binary while preserving state:

```bash
PREFIX="$HOME/.local" scripts/uninstall.sh --confirm
```

Back up `state.sqlite3` before deleting the state directory shown by `doctor`.
Unrelated hook handlers must remain untouched.

The `.codex-plugin` source manifest and native Codex adapter are development
previews, not public beta installation routes. No marketplace trust flow is
claimed for this release.
