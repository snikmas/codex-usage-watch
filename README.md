# Codex Usage Watch

Codex Usage Watch is a local, non-blocking usage-awareness tool. It tracks how
current Codex activity compares with the historical five-hour allowance and how
many weekly percentage points the same local window consumed.

It never blocks a prompt, never treats an estimate as an official quota, and
never stores prompt text, responses, tool arguments, or source code.

Typical native footer:

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
packaging smoke checks.

```bash
make test
make lint
make build
```

## Install and first setup

Install the standalone tracker without reading historical sessions:

```bash
PREFIX="$HOME/.local" scripts/install.sh
```

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

## Commands

```text
codex-5h setup [--preview|--skip-import|--import --confirm]
codex-5h status
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
the next structured observation starts a new window. `doctor` validates the
installed executable, state/schema, projection, session-directory access, and
hook configuration. `doctor --compat` performs the separate compatibility check.

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
- Linux is locally lifecycle-tested. macOS Rust checks pass in hosted CI.
  Windows is not yet an advertised platform because its current test job fails;
  installation lifecycle acceptance also remains outstanding outside Linux.

## Project status

This repository is an experimental public beta. See [CHANGELOG.md](./CHANGELOG.md)
for shipped behavior and the limitations above before recommending it to others.
