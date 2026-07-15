# Support matrix

This matrix is the single source of truth for version `0.1.0-beta.1`.

| Surface | Status | Evidence |
|---|---|---|
| Linux x86_64 standalone archive | Beta candidate, not released | Local exact-artifact lifecycle passed; public Ubuntu CI and external acceptance remain release gates |
| macOS Rust library and CLI | CI-configured preview | Rust and shell-lifecycle jobs are configured; no advertised artifact until public CI and a real user lifecycle pass |
| Windows Rust library and CLI | CI-configured build-only | Rust CI is configured; native installation is unsupported in this beta |
| Official Codex plus user hooks | Beta candidate, not released | Absolute-path hook install, doctor, backup, upgrade, and uninstall passed locally |
| Codex plugin marketplace install | Unsupported | No public marketplace submission or trust-onboarding claim |
| Native Codex footer and `/status` adapter | Development preview | Not present in standalone artifacts; see `NATIVE_ADAPTER.md` |

The minimum supported Rust version for source builds is 1.85. CI tests that
MSRV separately from the current stable toolchain. Platform status may only be
promoted after its named acceptance evidence exists; a green cross-platform
compile alone does not establish installation support.
