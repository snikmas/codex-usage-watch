# Codex Usage Watch

> **Experimental beta candidate.** Tested on Ubuntu 25.10 x86_64 with the
> checksummed standalone archive and Codex CLI 0.144.4.
> Other Linux distributions may work, but they have not been verified. macOS is
> preview-only, and Windows installation is unsupported.

Codex Usage Watch is a private, local pressure gauge for Codex usage. It turns
the weekly percentage already recorded by Codex into an estimate of one
historical five-hour allowance. It never blocks a prompt.

The number can be wrong. It is a local estimate, not official OpenAI quota,
billing, or account data. Use it for awareness, not as proof of what you used or
what you will be charged.

You are welcome to try the experimental beta in the tested environment and
report anything confusing or broken. Use a [bug report](https://github.com/snikmas/codex-usage-watch/issues/new?template=bug.yml),
a [compatibility report](https://github.com/snikmas/codex-usage-watch/issues/new?template=compatibility.yml),
or [private vulnerability reporting](https://github.com/snikmas/codex-usage-watch/security/advisories/new).
Before sharing diagnostics, review them and never attach Codex transcripts,
prompts, source code, `display.json`, `state.sqlite3`, or private local paths.

## What you will see

```text
5 hour estimate: 32% | 5.1 weekly points
```

- `fresh` means a recent supported observation was found.
- `stale` means the last observation is older than the freshness window.
- `unknown` means there is not enough compatible data yet. It does not mean 0%.
- Values above 100% remain visible and Codex continues normally.

The optional native footer is a separate development preview and is not in the
beta archive. The supported build does not add a permanent Codex status-line
item: use `codex-5h status` for an always-available reading. Trusted lifecycle
hooks show a session-start message and newly crossed warning thresholds; the
`Stop` hook is intentionally silent.

## Install the beta

There is no published beta artifact yet. Until the GitHub prerelease appears,
the recommended public action is to wait; source installation is for
contributors and is described separately below.

When `0.1.0-beta.1` is published, download its Ubuntu 25.10 x86_64 standalone
archive and `SHA256SUMS` into a new empty directory. Then run:

```bash
sha256sum -c SHA256SUMS
tar -xzf codex-usage-watch-0.1.0-beta.1-x86_64-unknown-linux-gnu.tar.gz
cd codex-usage-watch-0.1.0-beta.1-x86_64-unknown-linux-gnu
PREFIX="$HOME/.local" INSTALL_HOOKS=1 scripts/install.sh
PREFIX="$HOME/.local" scripts/verify-install.sh
```

The target name in the archive filename is a build identifier, not a promise
that every system using that target is compatible.

After installation:

1. Start or restart Codex.
2. Open `/hooks`.
3. Review the `SessionStart`, `UserPromptSubmit`, and `Stop` definitions. Each
   must call the expected absolute `codex-5h` path with a five-second timeout.
4. Trust all three definitions and start a fresh Codex session.
5. Run:

```bash
"$HOME/.local/bin/codex-5h" setup --skip-import
"$HOME/.local/bin/codex-5h" status
"$HOME/.local/bin/codex-5h" doctor
```

History import is optional. `setup --preview` reads filenames and metadata only;
transcript content is parsed only after explicit import consent.

For checksum details, backup/restore, upgrades, and recovery, use the complete
[installation guide](docs/INSTALL.md).

## Remove it

From the extracted, checksum-verified archive directory:

```bash
PREFIX="$HOME/.local" scripts/uninstall.sh --confirm
```

This removes only Codex Usage Watch hooks and its installed binary. It preserves
the local database. If the installed binary is already missing, the archive's
bundled binary safely removes the hooks. Without either binary, the script stops
with recovery steps instead of claiming that cleanup succeeded.

## Privacy and help

The tracker reads structured rate-limit metadata and timestamps. It does not
retain prompts, responses, tool arguments, or source code. Its SQLite database
and generated reports stay on this computer unless you choose to share a
reviewed support bundle.

```bash
codex-5h doctor --json
codex-5h doctor --support-bundle ./codex-usage-watch-support.json --confirm
```

Review any output before sharing it. See [privacy](docs/PRIVACY.md),
[support](docs/SUPPORT.md), and [security reporting](SECURITY.md).

## Source checkout for contributors

Source builds require Rust 1.85 or newer, Cargo, Bash, Python 3, and the same
Ubuntu 25.10 x86_64 environment for the currently verified installation path.

```bash
make test
make lint
PREFIX="$HOME/.local" scripts/install.sh
```

Source installation is not the recommended beta route. Contributors should run
the full acceptance gate before proposing a release change:

```bash
ALLOW_DIRTY=1 bash scripts/release-gate.sh
```

## Current limitations

- Only Ubuntu 25.10 x86_64 with the checksummed standalone archive and Codex CLI
  0.144.4 has completed the claimed user lifecycle.
- Other Linux distributions and architectures are unverified. macOS is a build
  and shell-lifecycle preview; Windows has build/test coverage only.
- The database has no retention or compaction policy yet and can grow over time.
- Long-running multi-window dogfood and independent clean-machine feedback are
  beta follow-up work, so this release must not be described as stable.
- Release publication recovery, plugin-validator ownership, and automated
  dependency security updates remain deferred before a stable release.
- Plugin marketplace installation and the native Codex footer are not supported
  beta routes.

Technical details live in [the support matrix](docs/SUPPORT.md),
[release policy](docs/RELEASE.md), [privacy model](docs/PRIVACY.md), and the
historical implementation notes under `notes/`.
