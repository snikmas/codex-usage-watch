# macOS preview

Apple Silicon (`aarch64-apple-darwin`) is the first planned macOS target. It is
still a preview until the exact published artifact passes the complete lifecycle
on a real Mac. Intel macOS is not a supported beta target.

The tracker uses the macOS application-data directory selected by the operating
system through the Rust `directories` API. `CODEX_USAGE_WATCH_HOME` can override
that location. Codex hooks remain in `$CODEX_HOME/hooks.json`, or `~/.codex` when
`CODEX_HOME` is unset.

For an Apple Silicon preview artifact, verify and exercise it from a clean
directory:

```bash
shasum -a 256 -c SHA256SUMS
tar -xzf codex-usage-watch-VERSION-aarch64-apple-darwin.tar.gz
cd codex-usage-watch-VERSION-aarch64-apple-darwin
bash scripts/verify-release-lifecycle.sh \
  ../codex-usage-watch-VERSION-aarch64-apple-darwin.tar.gz ../SHA256SUMS
```

The lifecycle script covers spaces and Unicode paths, checksum verification,
install, setup, status, refresh, history, analyze, doctor, all three protocol
handlers, backup, upgrade, rollback, uninstall, unrelated hooks, state
retention, and private Unix modes.

Real `/hooks` review and trust cannot be automated. Restart Codex, inspect and
trust `SessionStart`, `UserPromptSubmit`, and `Stop`, then use a content-free
test turn. `doctor` can validate configuration and executable paths; it cannot
claim that Codex trust was granted.

## Troubleshooting

- If macOS blocks execution, confirm the checksum and artifact source before
  changing any security setting. This project does not currently publish a
  signed or notarized binary.
- Quote every path that contains spaces or Unicode characters.
- If `codex-watch` is not on `PATH`, invoke the exact installed path reported by
  the installer.
- The normal Codex CLI does not gain custom `/status` or `/statusline` rows.
  Those screenshots use a separate optional custom build.
