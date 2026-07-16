# Codex Usage Watch

Codex Usage Watch is a local, non-blocking usage-awareness tool. It tracks how
current Codex activity compares with the historical five-hour allowance and how
many weekly percentage points the same local window consumed.

It never blocks a prompt, never treats an estimate as an official quota, and
never stores prompt text, responses, tool arguments, or source code.

Development-preview native footer (not included in the beta artifact):

```text
gpt-5.6-sol medium · ~/work/project · 5h est 32% · week +5.1%
```

Usage above 100% remains numeric and Codex continues normally.

## What is included

- deterministic five-hour accounting from weekly movement;
- SQLite state with concurrent-writer and migration safety;
- incremental transcript cursors with truncate/rotation detection;
- versioned `display.json` for the optional thin Codex TUI adapter;
- fail-open `SessionStart`, `UserPromptSubmit`, and `Stop` hooks;
- real 300-minute and 10080-minute window detection in either rate-limit slot;
- plan/model/tier/schema/version-partitioned calibration evidence;
- movement-quality classification and a movement-weighted robust median;
- explicit confidence states and stable calibration IDs stored on every window;
- once-per-identity compatibility checks and `doctor --compat`;
- consent-first historical setup/import;
- reproducible install, verify, upgrade, backup, and rollback scripts.

## Estimate priority and calibration safety

The selected five-hour value follows this order:

1. a fresh real server 300-minute window, when Codex provides one;
2. an explicitly approved compatible personal profile;
3. the bundled `15.8` baseline only for an identified Plus plan;
4. otherwise unknown, while weekly cost can remain available.

`15.8` is historical Plus-derived evidence. It is not presented as a validated
default for Pro, Team, Enterprise, or an unknown plan.

Completed paired windows are classified by five-hour movement:

| Movement | Quality | Calibration influence |
|---:|---|---|
| below 25 points | ignored | retained with an exclusion reason |
| 25-49 | low | retained, excluded from estimator |
| 50-79 | useful | weighted evidence |
| 80-100 | high | weighted evidence |

The estimator uses a movement-weighted median. Five useful/high-quality samples
create a reviewable candidate, not automatic validation. Replacement also needs
approximately 10% persistent drift, acceptable spread, fresh compatible
evidence, and confirmation in a later analysis. Nothing is auto-applied.

Confidence states are `baseline`, `personal_preliminary`, `personal_candidate`,
`personal_validated`, `inherited_unvalidated`, and `unsupported`.

## Build and test

Requirements: a current stable Rust toolchain, Cargo, Bash, and Python 3 for the
packaging smoke checks. Source builds support Rust 1.85 and newer.

```bash
make test
make lint
make build
```

## Install and first setup

The supported beta distribution is the checksummed Linux x86_64 standalone
archive plus explicit user hooks. Follow [docs/INSTALL.md](docs/INSTALL.md) for
the released-artifact path. From a trusted source checkout, install the tracker
without reading historical sessions with:

```bash
PREFIX="$HOME/.local" scripts/install.sh
```

Expected result:

```text
Installed codex-5h tracker-0.1.0-beta.1 at /home/you/.local/bin/codex-5h
Next: /home/you/.local/bin/codex-5h setup --preview
```

If that directory is not on your shell `PATH`, use the printed absolute path.
Installed hooks also use this absolute executable path and do not depend on the
Codex process inheriting your interactive shell environment.

Preview what an optional import would inspect. This reads filenames and file
metadata only; it does not open transcript contents or create tracker state:

```bash
codex-5h setup --preview
```

Run interactive setup to see the same preview and choose whether to import:

```bash
codex-5h setup
```

You can explicitly skip import and start from future observations:

```bash
codex-5h setup --skip-import
```

Only after consent does setup parse transcript content. The parser extracts
timestamps, rate-limit windows, model, plan, service tier, schema shape, and
Codex version. All other event content is ignored and not retained.

Install the hooks explicitly:

```bash
codex-5h install --confirm
```

Installation alone never scans historical session files. Ordinary SessionStart
also performs only cheap state/identity checks; live transcript cursors advance
on lifecycle events that provide a transcript path.

Hook configuration is not the same as hook trust. After installing or changing
the hooks:

1. Start or restart Codex.
2. Open `/hooks`.
3. Inspect the hook source and the exact `SessionStart`, `UserPromptSubmit`, and
   `Stop` commands. Each must use the expected absolute `codex-5h` path and a
   five-second timeout.
4. Trust the reviewed definitions, then start a fresh Codex session.

Codex records trust against the current hook definition. A new or changed
non-managed hook receives a different hash and may be skipped until you review
and trust it again. `codex-5h doctor` can prove that the configuration is
well-formed and path-valid, but it deliberately reports that trust must be
confirmed inside Codex.

## Commands

```text
codex-5h setup [--preview|--skip-import|--import --confirm]
codex-5h status
codex-5h status --json
codex-5h refresh [--transcript PATH]
codex-5h history [--json]
codex-5h analyze [--json]
codex-5h reset --confirm
codex-5h doctor
codex-5h doctor --compat [--refresh-releases]
codex-5h calibration apply WEEKLY_POINTS --confirm
codex-5h backup DESTINATION.sqlite3 --confirm
codex-5h install --confirm
codex-5h uninstall --confirm
```

`analyze` reports identity, calibration ID, confidence, sample quality, weighted
median, quartiles/range, prediction error, evidence period, drift, and why a
change is or is not recommended. SessionStart writes an initial report, a weekly
report when due, and an early report after five new qualifying windows.

Release metadata refresh is optional, uses the official GitHub release API, is
cached for at least 24 hours, and treats returned prose only as data. It cannot
execute instructions or modify calibration.

`history` lists the 20 newest local windows and manual-control audit events.
`reset --confirm` archives the current local window without deleting snapshots;
the next structured observation starts a new window. `doctor` reports executable,
state/schema, projection, session-directory, hook configuration/path, and basic
compatibility checks independently before returning failure. It cannot prove the
interactive Codex trust decision. `doctor --compat` provides the detailed
compatibility report.

`status --json` is the stable `codex-usage-watch.status.v1` machine contract.
`refresh` checks at most eight transcripts from the last two days, or exactly
one file supplied with `--transcript`; it never performs an unbounded history
scan. Run `codex-5h --help` for examples and exit-status documentation.

## Supported settings

Version 1 supports one user-facing attention setting:

```bash
export CODEX_USAGE_WATCH_THRESHOLDS="60,80,100"
```

Values are positive integer percentages, separated by commas. They are sorted
and deduplicated. Invalid values make normal commands fail with exit status 2
and hooks fail open. The default is `75,90,100`. Window duration, freshness,
calibration selection, database paths, and super-usage increments are not
user-tunable policy in version 1; `CODEX_USAGE_WATCH_HOME` only relocates state.

## Try it safely in an isolated environment

```bash
TEMP="$(mktemp -d)"
export PREFIX="$TEMP/prefix"
export CODEX_HOME="$TEMP/codex-home"
export CODEX_USAGE_WATCH_HOME="$TEMP/state"
export PATH="$PREFIX/bin:$PATH"

scripts/install.sh
codex-5h setup --skip-import
codex-5h install --confirm
codex-5h status
codex-5h history
codex-5h analyze
codex-5h doctor
codex-5h doctor --compat
codex-5h backup "$TEMP/usage-watch-backup.sqlite3" --confirm
codex-5h uninstall --confirm
```

Or run the complete automated lifecycle:

```bash
bash scripts/smoke-install.sh
```

## State, backup, upgrade, and rollback

Set `CODEX_USAGE_WATCH_HOME` to choose an explicit state directory. Otherwise
the platform-specific per-user application-data location is used. The important
files are:

- `state.sqlite3`: authoritative state and calibration history;
- `display.json`: replaceable TUI projection;
- `calibration-report.json`: latest scheduled analysis report;
- `release-metadata.json`: optional cached official release metadata.

Create a SQLite-consistent, integrity-checked backup:

```bash
scripts/backup-state.sh /safe/path/codex-usage-watch.sqlite3
```

For restore, first close Codex sessions and uninstall the hooks, back up the
current database, then replace `state.sqlite3` with the chosen backup. The next
tracker start runs forward-only migrations and regenerates projections. A newer
unknown database schema is rejected without mutation.

Upgrade by rerunning `scripts/install.sh`, followed by
`scripts/verify-install.sh`. Roll back hooks and the binary with:

```bash
scripts/uninstall.sh --confirm
```

Uninstall preserves tracker state. Unrelated hooks are preserved during both
installation and removal.

## Troubleshooting and recovery

- Missing data: run `codex-5h refresh`, then `codex-5h doctor`. A new install
  remains unknown until a structured live event or consented import exists.
- Stale projection: run `codex-5h refresh --transcript PATH`. Stale means no
  recent supported observation; it is never converted to zero.
- Command not found: invoke the absolute path printed by `scripts/install.sh`
  or add `$HOME/.local/bin` (or your chosen `PREFIX/bin`) to `PATH`.
- Missing, malformed, or moved hooks: rerun the installed binary's
  `install --confirm`, then repeat the `/hooks` review/trust flow and run `doctor`.
  Doctor requires all three well-formed handlers to point at the equivalent
  canonical executable path but does not claim they are trusted.
- Unsupported schema: run `codex-5h doctor --compat`. Tracking remains
  non-blocking and the estimate becomes unknown instead of guessing.
- Damaged hook configuration: stop Codex and restore
  `$CODEX_HOME/hooks.json.codex-5h.bak`, which is written before every changed
  hook file replacement.
- Restore a database: stop Codex, uninstall hooks, back up the current database,
  replace `state.sqlite3` with the chosen integrity-checked backup, reinstall
  hooks, and run `doctor`.
- Complete rollback: run `scripts/uninstall.sh --confirm`; after optionally
  backing up, remove the directory printed as `State directory` by `doctor`.
  The script intentionally does not delete state automatically.

For the exact local data flow and limits, read [docs/PRIVACY.md](docs/PRIVACY.md).
Security reports should follow [SECURITY.md](SECURITY.md).

## Platform and support policy

Linux x86_64 is the only supported standalone beta artifact. macOS is a
build/test preview pending a real user lifecycle run, and Windows is build-only
with no supported native installer. Plugin marketplace installation is not a
beta route. See the authoritative [support matrix](docs/SUPPORT.md),
[contribution guide](CONTRIBUTING.md), and [release policy](docs/RELEASE.md).

Public project links:

- [installation guide](https://github.com/snikmas/codex-usage-watch/blob/main/docs/INSTALL.md)
- [release artifacts](https://github.com/snikmas/codex-usage-watch/releases)
- [issues and support requests](https://github.com/snikmas/codex-usage-watch/issues)
- [private security advisories](https://github.com/snikmas/codex-usage-watch/security/advisories/new)
- [support matrix](https://github.com/snikmas/codex-usage-watch/blob/main/docs/SUPPORT.md)

## Optional native Codex fork

Most users need only the executable and lifecycle hooks. The native footer is a
development preview and is not distributed by this repository's public beta.

`scripts/build-codex-fork.sh` requires an explicit checkout, exact Git ref,
release suffix, and output directory. It refuses to change the checkout, runs
focused TUI tests, builds the binary, names it with the suffix, and writes build
identity plus SHA-256 evidence. Tracker and fork release identifiers remain
independent.

Create standalone versioned tracker artifacts plus `SHA256SUMS` with:

```bash
make release
```

Before a native adapter can be distributed, a maintainer must publish its source
or a versioned patch, rebase it onto the chosen upstream ref, and rerun the
focused Codex TUI suite. This repository does not silently mutate or rebase
another repository.

## Current limitations and remaining real-world work

- The deterministic implementation and fixtures are complete, but several
  elapsed real five-hour windows still need observation-mode usability study
  before threshold/presentation tuning.
- The native Codex footer adapter is not included in the public repository or
  release artifacts. The standalone CLI and hooks are the supported beta path.
- The richer native `/usage` section remains later work; the existing optional
  adapter covers the footer and `/status`.
- Optional remote release checks require `curl`; all normal tracking works
  offline and does not need network access.
- Linux is locally lifecycle-tested. macOS has hosted Rust and shell-lifecycle
  gates but still needs a real user acceptance run. Windows has build/test CI
  only and no supported installation path.
- A clean-machine external tester using only the public instructions and exact
  release candidate artifact is still required before the beta may be tagged
  or recommended publicly.

## Project status

This repository is an experimental beta candidate. It must not be tagged or
recommended publicly until the external acceptance gate in
[docs/RELEASE.md](docs/RELEASE.md) is complete. See
[CHANGELOG.md](./CHANGELOG.md) for candidate behavior and the limitations above.
