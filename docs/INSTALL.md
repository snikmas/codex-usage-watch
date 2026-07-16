# Standalone beta installation

The only supported beta distribution is the checksummed standalone Linux
x86_64 archive. It contains `codex-5h`, documentation, and explicit install,
verification, and uninstall scripts. It does not replace the official Codex
binary and does not include the native footer adapter.

## Install and verify

Download exactly the Linux x86_64 archive and `SHA256SUMS` from the same beta
release into a blank directory. `SHA256SUMS` intentionally contains only the
archive, so the documented check does not require the separately published
source crate:

```bash
sha256sum -c SHA256SUMS
tar -xzf codex-usage-watch-0.1.0-beta.1-x86_64-unknown-linux-gnu.tar.gz
cd codex-usage-watch-0.1.0-beta.1-x86_64-unknown-linux-gnu
python3 -m json.tool BUILD-INFO.json >/dev/null
python3 -m json.tool SBOM.spdx.json >/dev/null
PREFIX="$HOME/.local" INSTALL_HOOKS=1 scripts/install.sh
PREFIX="$HOME/.local" scripts/verify-install.sh
```

`BUILD-INFO.json` records the source revision, target, Rust/Cargo versions, and
`Cargo.lock` digest used for the archive. `SBOM.spdx.json` is the matching SPDX
2.3 software bill of materials. The release gate verifies both files from the
extracted archive; neither file is a cryptographic signature.

Complete the Codex trust flow after installation:

1. Start or restart Codex and open `/hooks`.
2. Inspect the hook source and the exact `SessionStart`, `UserPromptSubmit`, and
   `Stop` definitions. Each must call the absolute installed `codex-5h` path and
   use a five-second timeout.
3. Trust the reviewed definitions, then start a fresh Codex session.

Codex records trust for the current hook-definition hash. A newly installed or
changed non-managed hook may be skipped until it is reviewed and trusted again.
Installation saves a recoverable `hooks.json.codex-5h.bak` before changing an
existing hooks file. `codex-5h doctor` validates configuration and executable
paths but intentionally says that trust must be confirmed inside Codex.

Initialize without importing history:

```bash
"$HOME/.local/bin/codex-5h" setup --skip-import
"$HOME/.local/bin/codex-5h" status
"$HOME/.local/bin/codex-5h" doctor
```

History import is optional and consent-gated; use `setup --preview` first.

On Unix, startup creates or repairs the tracker state directory to `0700` and
tracker-owned state files to `0600`. It does not chmod the selected parent
directory. If `doctor` reports a permission-related I/O failure, verify that the
current user owns the state directory, then rerun `doctor`; do not solve it by
making the directory group- or world-readable. Windows permission bits are not
claimed as protection, and Windows installation is unsupported in this beta.

## Backup and restore

Create and integrity-check a backup with the helper included in the archive:

```bash
PREFIX="$HOME/.local" scripts/backup-state.sh /safe/path/codex-usage-watch.sqlite3
```

To restore, close Codex, remove only this tool's hooks, copy the verified backup
over the `state.sqlite3` shown by `doctor` after removing its `-wal` and `-shm`
sidecars, reinstall the hooks, repeat `/hooks` review if Codex requests it, and
run `scripts/verify-install.sh`. Preserve the current database separately until
the restored state passes verification.

## Upgrade, disable, and uninstall

To upgrade, verify the new checksum, extract the new archive, retain the prior
verified archive/binary for rollback, and rerun the new archive's installer and
verifier. The SQLite state is migrated forward and retained. To roll back, run
`codex-5h uninstall --confirm`, restore the prior verified `codex-5h` binary,
run `codex-5h install --confirm`, repeat `/hooks` review when requested, and run
the prior archive's `scripts/verify-install.sh`.

To temporarily disable the tracker, remove only its hooks:

```bash
"$HOME/.local/bin/codex-5h" uninstall --confirm
```

To remove both hooks and the installed binary while preserving state:

```bash
PREFIX="$HOME/.local" scripts/uninstall.sh --confirm
```

Back up `state.sqlite3` before deleting the state directory shown by `doctor`.
Unrelated hook handlers must remain untouched. Every script named in this guide
is included in the standalone archive.

The `.codex-plugin` source manifest and native Codex adapter are development
previews, not public beta installation routes. No marketplace trust flow is
claimed for this release.
