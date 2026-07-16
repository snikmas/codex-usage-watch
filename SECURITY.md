# Security policy

Codex Usage Watch is local attention software. It is not an authorization,
quota, billing, or policy-enforcement boundary.

## Report a vulnerability privately

Use GitHub's [private vulnerability reporting form](https://github.com/snikmas/codex-usage-watch/security/advisories/new).
It is enabled for this repository and sends the report privately to the
maintainer. A public issue is not an acceptable security-reporting route.

Include the affected version, Ubuntu/Codex version when relevant, a minimal
reproduction, and reviewed `codex-5h doctor --json` output. Never attach Codex
transcripts, prompts, responses, source code, credentials, local paths,
`display.json`, or a state database.

The project does not promise a response deadline. Security fixes target the
latest beta candidate and latest published beta; older betas should be upgraded
or uninstalled.

## Local recovery

If hook configuration is damaged, stop Codex and inspect
`$CODEX_HOME/hooks.json.codex-5h.bak` before restoring it. Then restart Codex,
review `/hooks`, and run `codex-5h doctor`.

`codex-5h uninstall --confirm` structurally removes only matching Codex Usage
Watch handlers. The archive uninstaller preserves the database. See
[installation recovery](docs/INSTALL.md) and the [privacy model](docs/PRIVACY.md).
