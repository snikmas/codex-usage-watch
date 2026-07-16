# Install the experimental beta

The verified public route is the checksummed standalone archive on Ubuntu 25.10
x86_64 with Codex CLI 0.144.4. Other Linux distributions may work but have not
been verified. The `x86_64-unknown-linux-gnu` filename is a Rust build target,
not a general compatibility promise.

`0.1.0-beta.1` is still a candidate. Do not follow an unofficial download link;
wait for the prerelease on the repository's [Releases page](https://github.com/snikmas/codex-usage-watch/releases).

## Install and verify

Download the archive and `SHA256SUMS` from the same prerelease into a new empty
directory. Check the archive before running anything from it:

```bash
sha256sum -c SHA256SUMS
tar -xzf codex-usage-watch-0.1.0-beta.1-x86_64-unknown-linux-gnu.tar.gz
cd codex-usage-watch-0.1.0-beta.1-x86_64-unknown-linux-gnu
python3 -m json.tool BUILD-INFO.json >/dev/null
python3 -m json.tool SBOM.spdx.json >/dev/null
PREFIX="$HOME/.local" INSTALL_HOOKS=1 scripts/install.sh
PREFIX="$HOME/.local" scripts/verify-install.sh
```

The archive contains `codex-5h`, this documentation, install/uninstall helpers,
build identity, and an SPDX dependency list. It does not replace Codex and does
not contain the optional native footer.

## Trust the hooks in Codex

Installation writes three user hooks, but non-managed hooks do not run until
you approve their exact definitions.

1. Start or restart Codex and open `/hooks`.
2. Review `SessionStart`, `UserPromptSubmit`, and `Stop`.
3. Confirm that each command uses the absolute installed `codex-5h` path and a
   five-second timeout.
4. Trust all three definitions, then start a fresh session.

Changing a definition changes its trust hash. Codex should ask you to review it
again. `codex-5h doctor` validates configuration and executable paths, but only
Codex `/hooks` can show the interactive trust decision.

## First use

Start without reading old transcripts:

```bash
"$HOME/.local/bin/codex-5h" setup --skip-import
"$HOME/.local/bin/codex-5h" status
"$HOME/.local/bin/codex-5h" doctor
```

An `unknown` result means that no recent compatible usage observation has been
seen yet; it is not zero. A `stale` result means the last usable observation is
too old. Submit a prompt after trusting the hooks or run `codex-5h refresh`.

History import is optional. `setup --preview` reads only filenames and metadata.
Only an explicitly confirmed import parses transcript content, and the tracker
retains only allowlisted usage metadata and cursors.

## Backup and restore

Create an integrity-checked SQLite backup:

```bash
PREFIX="$HOME/.local" scripts/backup-state.sh /safe/path/codex-usage-watch.sqlite3
```

To restore, close Codex, uninstall the hooks, preserve the current database,
remove its `-wal` and `-shm` sidecars, replace `state.sqlite3` with the verified
backup, reinstall the hooks, repeat `/hooks` review if requested, and run
`scripts/verify-install.sh`.

On supported Unix systems the tracker directory is `0700`; its database,
projection, reports, and backups are `0600`. Do not fix ownership trouble by
making those files group- or world-readable.

## Upgrade, disable, and uninstall

For an upgrade, verify and extract the new archive, keep the prior verified
archive for rollback, then rerun its installer and verifier.

Temporarily remove only the hooks:

```bash
"$HOME/.local/bin/codex-5h" uninstall --confirm
```

Remove the hooks and binary while preserving state:

```bash
PREFIX="$HOME/.local" scripts/uninstall.sh --confirm
```

Run that command from the extracted, checksum-verified archive. If the installed
binary is missing, the bundled archive binary performs safe structural hook
removal. If neither binary is available, the script does not edit `hooks.json`;
it reports partial cleanup and tells you to verify and extract the matching
archive. Unrelated hooks are never removed. Repeating uninstall is safe.

The `.codex-plugin` manifest and native Codex adapter are development previews,
not beta installation routes.
