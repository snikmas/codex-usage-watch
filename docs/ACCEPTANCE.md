# Acceptance evidence

This file records only sanitized product evidence. It must never contain Codex
session text, transcript paths, account identifiers, prompts, responses,
reasoning, tool arguments, command output, or source code.

The project is complete for its frozen Ubuntu experimental-beta scope. The open
records below are optional evidence for stronger accuracy, stability, or
platform-support claims; they do not represent missing product features.

## Evidence format

Longitudinal and tester observations use
[`acceptance-record-v1.schema.json`](acceptance-record-v1.schema.json). Validate
each record before summarizing it:

```bash
python3 scripts/validate-acceptance-record.py RECORD.json
```

Independent Ubuntu testers should copy
[`acceptance-record-stage15.example.json`](acceptance-record-stage15.example.json),
replace every synthetic environment/reading value with their content-free
observation, and use the stricter Stage 15 gate:

```bash
python3 scripts/validate-acceptance-record.py \
  --require-stage 15 RECORD.json
```

The fixed allowlist intentionally permits timezone-aware chronological
timestamps, numeric usage readings, controlled artifact and compatibility
identifiers, scenarios, warning milestones, and controlled usability note codes
only. Ground-truth error is accepted only when the record says the comparison is
identity-safe and the absolute error is mathematically consistent.

## Stage 14: longitudinal accuracy and understanding

Status: **open — evidence collection is time-bound**.

No naturally elapsed beta windows are claimed in this repository yet. The
following coverage must be filled from validated records; synthetic fixtures
and replay tests do not count.

| Window | Normal/low activity | Stale/missing | Threshold | Concurrent | Weekly reset | Ground truth/error |
|---|---|---|---|---|---|---|
| 1 | open | open | open | open | open | open |
| 2 | open | open | open | open | open | open |
| 3 | open | open | open | open | open | open |
| 4 | open | open | open | open | open | open |
| 5 | open | open | open | open | open | open |

Required interpretation review remains open for `estimated`, `weekly cost`,
`stale`, `unknown`, values over 100%, and the 75/90/100/super-usage notices.
Wording and default thresholds must not be changed merely to close this table;
changes require collected evidence and regression tests.

## Stage 15: independent Ubuntu 25.10 acceptance

Status: **open — an independent tester has not completed the lifecycle**.

Give the tester only the published archive, its `SHA256SUMS`, README, and the
documents and scripts inside the archive. On Ubuntu 25.10 x86_64, first run:

```bash
tar -xzf codex-usage-watch-VERSION-x86_64-unknown-linux-gnu.tar.gz
cd codex-usage-watch-VERSION-x86_64-unknown-linux-gnu
bash scripts/verify-release-lifecycle.sh \
  ../codex-usage-watch-VERSION-x86_64-unknown-linux-gnu.tar.gz ../SHA256SUMS
```

Then perform the real-user checks that an isolated script cannot prove:

1. Verify the checksum before extraction and record it in a validated acceptance
   record with `observer_role` set to `independent_tester`.
2. Install without private maintainer help; review and trust all three commands
   in `/hooks`; restart Codex; observe `SessionStart`, `UserPromptSubmit`, and
   `Stop` during a real content-free test turn.
3. Run setup, status, refresh, history, analyze, doctor, backup, upgrade,
   rollback, and uninstall. Confirm saved state remains and unrelated hooks do
   not change.
4. Record any unpublished help with the controlled
   `needed_unpublished_help` note code. A run with that code does not pass.
5. Check that the privacy wording and Unix file modes are understandable.

The Stage 15 validator passes only an independent tester on Ubuntu 25.10 x86_64
using a published release, all three real hooks, every required lifecycle step,
public documentation only, and no unpublished help. The automated
`Clean Ubuntu 25.10 lifecycle` CI job exercises the artifact in a blank
`ubuntu:25.10` container, but it does not replace the human tester.

The full lifecycle must be repeated after any blocking fix. The successful
artifact checksum, OS, architecture, Codex version, and artifact version remain
open until that external run occurs.

## Stage 16: Apple Silicon macOS acceptance

Status: **preview — CI and artifact machinery are not real-Mac acceptance**.

The first intended target is `aarch64-apple-darwin`. Intel macOS remains
preview-only. CI must build the Apple Silicon archive, verify its checksum,
provenance, SBOM, privacy allowlist, package documentation, quoted/Unicode paths,
Unix modes, and the isolated lifecycle.

Before macOS can be labeled supported beta, a real Apple Silicon Mac must repeat
the published-artifact lifecycle in a clean directory, review/trust all three
hooks in Codex, and complete a real content-free turn. A non-maintainer macOS
tester is still required before broad recommendation.
