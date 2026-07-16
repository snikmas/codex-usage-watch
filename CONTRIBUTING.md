# Contributing

Codex Usage Watch is a small, privacy-sensitive Rust project. Before opening a
change, describe the user-visible problem and keep accounting, transcript
parsing, and hook behavior covered by deterministic tests.

Run the local gates before submitting a pull request:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
bash scripts/smoke-install.sh
```

For transcript parser changes, install `cargo-fuzz` and run the bounded parser
harness separately (it is not part of ordinary Cargo tests):

```bash
(cd fuzz && cargo fuzz run transcript)
```

Never add a real transcript as a fuzz seed or crash artifact. Reduce findings to
synthetic metadata-only cases before committing them.

Bug reports should include the command, operating system, tracker version,
expected behavior, and sanitized diagnostics. Never attach Codex transcripts,
prompts, responses, source code, `state.sqlite3`, or `display.json` without
reviewing them yourself. Security reports belong in the private process in
[SECURITY.md](SECURITY.md), not a public issue.

The project follows the release policy in [docs/RELEASE.md](docs/RELEASE.md).
