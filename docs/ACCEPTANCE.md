# Beta candidate acceptance record

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
- The Rust 1.85 MSRV check passed locally. The stable suite passed 55 automated
  tests with the manual real-transcript test excluded from the default suite.

This evidence does not complete the release gate. Still required:

- several naturally elapsed observation-mode five-hour windows with recorded
  notice/threshold usability feedback;
- a real macOS user lifecycle run before macOS is advertised;
- green public CI on the exact release commit; and
- one clean-machine external tester using only public instructions, checksum,
  and release candidate archive.

Until all four exist, the candidate must not be tagged or recommended publicly.
