# Security policy

Codex Usage Watch is local attention software, not an authorization, quota,
billing, or policy-enforcement boundary. Users who can modify the current
account's files can change its hooks, database, projection, and thresholds.

## Reporting a vulnerability

Do not include transcripts, prompts, source code, credentials, or a copy of a
real state database in a public issue. Until a dedicated security address is
published, use the repository host's private security-advisory feature. Include
the affected version, operating system, minimal reproduction, and the result of
`codex-5h doctor`; redact home paths and local identifiers.

Security fixes are accepted for the latest beta candidate and latest published
beta. This small project does not promise a response deadline. Unsupported old
beta builds should be upgraded or uninstalled; the current platform scope is in
[docs/SUPPORT.md](docs/SUPPORT.md).

## Local recovery

If hook configuration was damaged, stop Codex, copy
`$CODEX_HOME/hooks.json.codex-5h.bak` over `$CODEX_HOME/hooks.json`, inspect the
result, and restart Codex. `codex-5h uninstall --confirm` removes only matching
Codex Usage Watch handler objects. It deliberately preserves the database;
follow the complete-removal instructions in the README if local data must also
be erased.

See [docs/PRIVACY.md](docs/PRIVACY.md) for the data-flow and threat model.
