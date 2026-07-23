# Codex Usage Watch

A small personal project that estimates how much of the old five-hour Codex
allowance you have used. It reads the weekly percentage already recorded by
Codex and turns the change into a local estimate:

```text
5h est 158% · week +25.0%
```

- `5h est 158%` means about 1.58 times the old five-hour allowance.
- `week +25.0%` means weekly usage increased by 25 percentage points during
  this five-hour window.
- The estimate can go above 100%. Nothing is blocked.

This is only a local estimate, not official OpenAI usage or billing data.

## Project status

**Completed personal project.** The planned tracker is feature-complete and its
scope is now frozen. It remains an experimental beta because the number is a
local estimate rather than an official account limit. Future work is limited to
important fixes and compatibility maintenance; the unfinished ideas in
`notes/plan.md` are not part of the completed scope.

The latest public release is
[`v0.1.0-beta.1`](https://github.com/snikmas/codex-watch/releases/tag/v0.1.0-beta.1).
The core tracker, reset-aware accounting, privacy controls, packaging, upgrade
and rollback checks, and automated Ubuntu/macOS artifact lifecycles are
implemented. Optional long-term and independent-user observations remain in
[acceptance evidence](docs/ACCEPTANCE.md); they are useful follow-up evidence,
not unfinished product features.

## Supported systems

| System | Status | What that means |
|---|---|---|
| Ubuntu 25.10 x86_64 | **Supported experimental beta** | Prebuilt release archive, installer, hooks, upgrade, rollback, backup, and uninstall lifecycle are tested. |
| Apple Silicon macOS | **Preview** | The project builds and its automated lifecycle passes, but the published artifact has not completed a real-Mac acceptance run. |
| Intel macOS | **Source preview only** | No prebuilt release artifact or support promise. |
| Windows | **Unsupported** | Rust build/tests run in CI, but there is no native installer, artifact, hook lifecycle, or WSL support claim. |
| Other Linux systems | **Unsupported/best effort** | They may work from source, but only the exact Ubuntu target above is claimed. |

Codex CLI is required on every system. “Supported” here describes the tested
project lifecycle, not the accuracy of an official OpenAI quota value.

## Four ways to see the output

### 1. Terminal

```bash
codex-watch status
```

![codex-watch status in a terminal](docs/images/terminal-status.png)

Other useful commands:

```bash
codex-watch refresh     # look for newer usage data
codex-watch history     # show recent five-hour windows
codex-watch analyze     # show how the estimate was calculated
codex-watch doctor      # check the local setup
```

### 2. Codex status line

```text
5h est 158% · week +25.0%
```

![Codex Usage Watch in the Codex status line](docs/images/statusline.png)

The normal Codex CLI cannot add this project to `/statusline`. The screenshot
uses a small custom Codex build with the `local-five-hour-limit` item. This part
is optional and is not installed by the setup below.

### 3. Codex `/status`

![Codex Usage Watch details in the custom Codex status screen](docs/images/codex-status.png)

The same custom build adds **Five-hour estimate** and **Weekly cost** to
`/status`. The normal Codex CLI does not show these rows, so use
`codex-watch status` for the same details.

### 4. Hook messages inside Codex

![Automatic Codex Usage Watch hook notice](docs/images/hook-notice.png)

The hooks show the current estimate when Codex starts and a short message when
you cross a warning level. The `Stop` hook saves the newest observation
silently. If a hook fails, Codex continues normally.

`/hooks` is only where you review and trust the hooks; it is not another status
screen.

## Install

The published experimental beta targets Ubuntu 25.10 x86_64. For other systems,
follow the status table above rather than assuming that a successful build means
the complete installation and hook lifecycle is supported.

For the Ubuntu beta, download the x86_64 archive and `SHA256SUMS` from the same
[GitHub release](https://github.com/snikmas/codex-watch/releases) into a clean
directory. Verify before extracting:

```bash
sha256sum -c SHA256SUMS
tar -xzf codex-usage-watch-VERSION-x86_64-unknown-linux-gnu.tar.gz
cd codex-usage-watch-VERSION-x86_64-unknown-linux-gnu
PREFIX="$HOME/.local" INSTALL_HOOKS=1 scripts/install.sh
```

The archive contains a prebuilt binary, so this path does not require Rust or
Git. The installer puts `codex-watch` in `~/.local/bin`, adds three Codex hooks,
does not replace Codex, and does not need `sudo`.

Contributors can instead install from source with Rust 1.85 or newer:

Clone the project and run:

```bash
git clone https://github.com/snikmas/codex-watch.git
cd codex-watch
make test
make lint
PREFIX="$HOME/.local" INSTALL_HOOKS=1 scripts/install.sh
```

Start tracking from now:

```bash
"$HOME/.local/bin/codex-watch" setup --skip-import
"$HOME/.local/bin/codex-watch" status
```

Then restart Codex, open `/hooks`, review and trust `SessionStart`,
`UserPromptSubmit`, and `Stop`, and start a new Codex session. You can check the
setup with:

```bash
"$HOME/.local/bin/codex-watch" doctor
```

If `codex-watch` is not found in a new terminal, either use the full path above
or add `~/.local/bin` to your `PATH`.

## Configure

The simplest configuration starts tracking from the moment you install:

```bash
codex-watch setup --skip-import
codex-watch doctor
codex-watch status
```

Restart Codex, open `/hooks`, inspect and trust `SessionStart`,
`UserPromptSubmit`, and `Stop`, then start a fresh Codex session. The tracker
cannot approve its own hooks; `doctor` verifies their paths and definitions.

Optional environment settings:

| Setting | When it applies | Purpose |
|---|---|---|
| `CODEX_HOME` | Runtime and installer | Use a Codex configuration directory other than `~/.codex`. |
| `CODEX_USAGE_WATCH_HOME` | Runtime and installer | Store the SQLite database, display projection, and report in a custom directory. |
| `CODEX_USAGE_WATCH_THRESHOLDS` | Runtime | Replace the default `75,90,100` warning levels with comma-separated positive integers, for example `70,85,100`. |
| `PREFIX` | Installer scripts | Choose the installation prefix; default is `~/.local`. |
| `INSTALL_HOOKS=1` | `scripts/install.sh` only | Add the three Codex hook definitions during installation. |

Keep `CODEX_HOME` and `CODEX_USAGE_WATCH_HOME` consistent across installation,
normal commands, backup, upgrade, and uninstall. If you customize warning
thresholds, set the variable in the environment that starts both Codex and
`codex-watch`. Run `codex-watch doctor --json` when diagnosing a configuration.

## Optional independent Ubuntu acceptance

An independent tester should use only the published archive, `SHA256SUMS`,
README, and files packaged inside the archive. After completing the full
lifecycle and a content-free real Codex turn, copy
`docs/acceptance-record-stage15.example.json`, replace its synthetic values, and
run:

```bash
python3 scripts/validate-acceptance-record.py \
  --require-stage 15 RECORD.json
```

This strict check rejects maintainer evidence, local/CI artifacts, missing
checksum or lifecycle steps, untrusted/missing real hooks, and any run that
needed unpublished help. Passing automation alone does not satisfy the
independent-user gate.

## Optional history import

By default, `setup --skip-import` starts from now and does not read old
sessions. To preview the older session files it can use:

```bash
codex-watch setup --preview
```

To import their usage metadata:

```bash
codex-watch setup --import --confirm
```

The tracker keeps usage metadata, not prompts, responses, tool arguments, or
source code.

## Backup, upgrade, and rollback

Create an integrity-checked backup before an upgrade and keep the previous
verified binary:

```bash
codex-watch backup "$HOME/codex-usage-watch-backup.sqlite3" --confirm
cp "$HOME/.local/bin/codex-watch" ./codex-watch.previous
```

Verify and extract the new release, then run its `scripts/install.sh` exactly as
in the install section. It preserves the state database and unrelated hooks.
To roll back with the saved verified binary:

```bash
codex-watch uninstall --confirm
install -m 0755 ./codex-watch.previous "$HOME/.local/bin/codex-watch"
"$HOME/.local/bin/codex-watch" install --confirm
"$HOME/.local/bin/codex-watch" doctor
```

Database migrations are forward-only. If the older binary rejects the upgraded
database, stop Codex and restore the pre-upgrade SQLite backup before running
the older binary. Do not try to make an older binary open a newer schema. Keep
the backup until the restored binary passes `doctor`.

## How the estimate works

Codex writes structured five-hour and weekly rate-limit snapshots in
`token_count.rate_limits`. When a valid 300-minute server window is present,
Codex Usage Watch uses its real `resets_at` epoch as the local window boundary.
For older or partial logs that expose only the weekly window, it retains the
original fallback: start a local five-hour window at the first observation and
convert positive weekly movement using the calibration value.

- `fresh` means recent usage data was found.
- `stale` means the newest data is old.
- `unknown` means there is not enough compatible data yet; it does not mean 0%.

The value is useful as a rough pressure gauge, not as an exact account limit.

### Reset-aware accounting

- A natural five-hour reset closes the old local window and starts the new
  server epoch. Warning milestones can fire again in that epoch.
- A natural weekly rollover does not close an unchanged five-hour window. The
  tracker keeps confirmed pre-reset growth and adds observed post-reset usage.
- If both the five-hour and weekly epochs restart before their advertised
  deadlines with matching inferred starts, history labels it `inferred full
  reset`. This is consistent with an earned reset, but it is not proof that the
  user selected `/usage` or that any particular server action caused it.
- Long gaps, missing reset timestamps, and one-sided early changes are labeled
  `ambiguous reset` instead of being presented with false certainty. Delayed
  observations from a superseded epoch are ignored.

Detection is delayed until Codex writes the first structured rate-limit
snapshot after the boundary. `codex-watch history` shows the inferred boundary
and honest label. Archived local windows, token-activity metadata, calibration
profiles, and user configuration are retained across server resets.

`codex-watch reset --confirm` is different: it archives only the current local
tracker window and records a manual control event. It cannot reset the server
quota and does not erase history.

## Privacy

Everything stays on your computer. The tracker reads structured rate-limit
metadata and timestamps from local Codex session files. It does not store your
prompts, responses, reasoning, tool arguments, command output, or source code.

Reset evidence contains only the previous/new five-hour and weekly reset
timestamps, the inferred boundary, classification/reason, and the sanitized
observation identity already used for deduplication. `doctor --json` and the
optional support bundle expose only aggregate reset-classification counts, not
raw transcript paths, account identifiers, prompts, responses, or database
contents.

State is stored under your local data directory in `codex-usage-watch`. You can
choose another location with `CODEX_USAGE_WATCH_HOME`.

## Remove it

Remove only the hooks and keep the command and saved data:

```bash
codex-watch uninstall --confirm
```

Remove the hooks and installed command while keeping the saved database:

```bash
PREFIX="$HOME/.local" scripts/uninstall.sh --confirm
```

Run the second command from the cloned project directory.

## Limitations

- The estimate depends on Codex's local session format and may become inaccurate
  if that format changes.
- A reset cannot be detected until a later structured `token_count` snapshot is
  written, and ambiguous evidence intentionally remains ambiguous.
- Apple Silicon macOS has automated build/lifecycle coverage but remains preview
  until real-Mac published-artifact acceptance succeeds. Intel macOS is
  source-preview-only with no artifact; Windows installation is unsupported.
- Longitudinal accuracy/usability evidence and independent Ubuntu acceptance
  remain optional follow-up evidence; the project makes no stable or broad
  platform claim.
- The `/statusline` and `/status` additions require the separate custom Codex
  build; the normal installation only provides the terminal command and hooks.
- The local database does not have automatic cleanup yet.

## Contributing

This is a personal project, but small issues and pull requests are welcome. Run
`make test` and `make lint` before submitting a change, and use synthetic test
data instead of real Codex transcripts.

MIT licensed.
