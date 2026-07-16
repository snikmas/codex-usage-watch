# Beta candidate acceptance record

## Stage 11 CI preparation

- Local Rust 1.85, hook/CLI tests, clippy, and `cargo deny check` pass after the
  Windows canonical-path fix and cargo-deny action upgrade.
- The remaining cargo-deny output is non-blocking: two intentional `getrandom`
  major lines and unused BSD-2-Clause, BSD-3-Clause, and ISC license allowances.
- Public exact-candidate CI evidence is not yet recorded. The prior main-branch
  run `29423465457` is historical failure evidence and must not be cited as the
  Stage 11 release gate.

## Stage 11 hook trust preparation

- Automated tests prove missing, malformed, canonical-path-mismatched, and
  configured/path-valid hook reporting, plus aggregate doctor output when state
  and hooks fail together.
- Installer output and public documentation require `/hooks` review and state
  that doctor cannot prove interactive trust.
- Real Codex trust and successful `SessionStart`, `UserPromptSubmit`, and `Stop`
  evidence is not yet recorded; the Stage 10 trust acceptance item is therefore
  reopened.

## Stage 11 exact-artifact preparation

Source-checkout evidence:

- Rust tests and packaging scripts pass from the repository checkout. This
  proves the build inputs, not the standalone user lifecycle.

Exact-artifact evidence:

- A blank temporary directory received only the generated archive and its
  archive-only `SHA256SUMS` file. Checksum verification, extraction, packaged
  Markdown/reference validation, installation, setup, hook path verification,
  backup/integrity, restore, upgrade, rollback, unrelated-hook preservation, and
  uninstall passed using extracted helpers only.
- A provisional local checksum is intentionally not promoted to release evidence.
  This acceptance record is itself packaged, so the final checksum must be
  recorded only for the frozen artifact used by the external tester and then
  compared with the separately downloaded published artifact.

## Stage 11 naming and truth synchronization

- The public repository was renamed to
  `https://github.com/snikmas/codex-usage-watch`; the remote, Cargo metadata,
  public links, product/package naming, and GitHub About text now agree.
- The repository-wide documentation gate passes version synchronization,
  relative Markdown and packaged-script references, old-public-URL detection,
  and support-matrix assertions. That source-only checker is not presented as
  an archive command.
- Stage 3 was checked against migration, transaction, concurrency, and projection
  tests. Stage 4 records the final `calibration apply` interface. Stage 11.5 now
  supplies per-command help, stable categorized exit behavior, read-only status,
  and serialized-contract coverage.

## Stage 11 maintainability and audit closure

- Persistence is separated into migration, transcript/cursor, window replay,
  calibration, compatibility, backup, and display modules. Every historical
  schema version migrates to the current schema in tests while retaining the
  data types available at that version.
- Table-driven accounting coverage exercises reordered events, duplicates,
  resets, and expiry. Compatibility-state creation is one immediate transaction;
  a simultaneous-writer test proves only one complete first-seen transition.
- Read-only status recomputes freshness in memory without rewriting its cached
  projection. Packaging clears stale staging directories, selects the exact
  version/target archive, and records a JSON boolean dirty-state marker.
- Every archive contains a generated SPDX 2.3 SBOM and build identity. These are
  provenance records, not signatures.

Local candidate verification on 2026-07-15 (Linux x86_64):

- The ignored real-transcript oracle passed against a separately inspected
  weekly rate-limit value. No transcript content or transcript identifier was
  copied into the repository.
- An isolated consented import read 429 real local transcript files, extracted
  12,669 structured observations, and retained no prompt/response/tool/source
  content. Human `status` and `analyze` output clearly labeled the value as an
  estimate, showed weekly points separately, and reported evidence limits.
- Two current real transcripts were refreshed concurrently into one temporary
  state database; both processes succeeded and produced one valid fresh
  projection.
- The checksummed candidate archive passed missing, stale, weekly-reset,
  concurrent-writer, above-100%, and wording checks using generated structured
  observations. Its lifecycle gate passed install, diagnosis, consistent
  backup, restore, upgrade, unrelated-hook preservation, uninstall, and state
  retention.
- The Rust 1.85 MSRV check passed locally. The stable suite passed 62 automated
  tests with the manual real-transcript test excluded from the default suite.

This evidence does not complete the release gate. Still required:

- several naturally elapsed observation-mode five-hour windows with recorded
  notice/threshold usability feedback;
- real Codex `/hooks` review/trust and successful three-event lifecycle evidence;
- green public CI on the exact release commit; and
- one clean-machine external tester using only public instructions, checksum,
  and release candidate archive.

Until all four exist, the candidate must not be tagged or recommended publicly.
macOS remains preview-only until it receives a separate real user lifecycle run.

## Stage 12 local hardening evidence (2026-07-16)

- Under `umask 022`, Linux lifecycle verification observed `0700` on the tracker
  state directory and `0600` on SQLite state, projection, and backup files.
  Unit coverage also repairs permissive database/report/cache modes while keeping
  the user-selected parent unchanged and preserving the existing database.
- The release privacy gate extracts both the standalone archive and Cargo crate,
  scans paths and contents, permits JSONL only for named synthetic fixtures,
  rejects databases/private markers/unexpected paths, and compares the archive
  against its exact documented manifest. A deliberately contaminated archive is
  rejected; the clean local candidate passes. Build path remapping removes the
  developer home path from the release binary.
- `codex-usage-watch.doctor.v1` JSON and the optional `0600` support bundle expose
  only version, OS/architecture, schema/projection state, hook-path validity,
  compatibility state, and stable issue codes. Tests reject transcript/state
  paths and sensitive field names from both outputs.
- Transcript ingestion retains bounded discovery and now caps each JSONL record
  at 1 MiB. Oversized input emits only a fixed diagnostic, later valid records
  remain readable, spaces/quotes work across Unix, non-UTF-8 bytes work on Unix
  filesystems that permit them, and replacement/truncate behavior remains
  deterministic. A separate cargo-fuzz target covers arbitrary
  transcript bytes without adding real transcripts as seeds.
- The dirty-tree implementation gate passed formatting, strict clippy, 69 Linux
  automated tests (one manual live test ignored), source/package lifecycle,
  exact-artifact behavior, extracted privacy/manifest/contamination checks,
  packaged docs, checksums, provenance, backup/restore/upgrade/rollback/uninstall,
  and Unix permission assertions.

This is local implementation evidence, not release evidence. The final candidate
SHA/checksum, green public CI, real Codex trust, naturally elapsed dogfood,
independent clean-machine acceptance, branch protection enforcement, tag, and
downloaded published-artifact verification remain intentionally unrecorded.
