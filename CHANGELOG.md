# Changelog

## 0.1.0-beta.1 - 2026-07-15

- Narrowed the experimental beta claim to Ubuntu 25.10 x86_64, checksummed
  standalone-archive installation, and Codex CLI 0.144.4; other environments
  remain explicitly unverified.
- Made generated Unix hook commands POSIX-safe for spaces, quotes, dollar signs,
  backticks, backslashes, and command-substitution text.
- Made bounded transcript discovery sort all daily candidates by modification
  time and stable path tie-break before truncation.
- Fixed concurrent-session reset-time jitter that could repeatedly add the same
  weekly percentage and display impossible five-hour estimates.
- Shortened the database-lock budget used by lifecycle hooks so a contended
  prompt fails open in hundreds of milliseconds instead of waiting about two
  seconds; normal CLI commands keep their longer retry budget.
- Made human-facing window, history, setup, and analysis times use concise local
  time while versioned JSON contracts remain machine-oriented UTC.
- Made archive uninstall truthful when the installed binary is missing, with a
  verified bundled-binary fallback and non-mutating recovery instructions.
- Added private vulnerability reporting, privacy-safe feedback routes, beta
  limitations, and beginner-focused public onboarding.
- Added local five-hour accounting, durable SQLite state, display projection,
  fail-open Codex hooks, and the optional thin TUI projection contract.
- Added incremental dual-window observation ingestion and transcript-generation
  continuity protection.
- Added plan-scoped calibration profiles, evidence quality classification,
  movement-weighted robust analysis, confidence states, drift confirmation,
  stable calibration IDs, and future-window-only approved changes.
- Added compatibility identities, inherited/unvalidated continuity, degraded
  unknown states, `doctor --compat`, and optional cached official release metadata.
- Added consent-first history setup, consistent backups, and reproducible local
  install, verify, upgrade, rollback, and optional fork-build recipes.
- Selected the standalone binary plus explicit user hooks as the supported beta
  distribution; the native Codex adapter remains a development preview.
- Added a declared Rust 1.85 MSRV, pinned CI actions, dependency policy checks,
  a truthful platform support matrix, and an exact-artifact beta release gate.
- Added an SPDX 2.3 dependency SBOM and build identity with source revision,
  target/toolchain details, and the locked dependency digest to every archive.
