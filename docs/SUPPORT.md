# Experimental beta support

This is the single source of truth for `0.1.0-beta.1`.

| Surface | Status | Evidence or limit |
|---|---|---|
| Ubuntu 25.10 x86_64 standalone archive | Experimental beta candidate | Exact checksummed-archive lifecycle tested locally with Codex CLI 0.144.4; published artifact recheck remains a release gate |
| Other Linux distributions or architectures | Unverified | They may work, but no compatibility claim is made from the Rust target name alone |
| macOS library and CLI | Preview | Hosted build and shell-lifecycle coverage only; no verified user artifact |
| Windows library and CLI | Build/test only | Native installation is unsupported |
| Official Codex plus user hooks | Experimental beta candidate | Three-event real trust/lifecycle evidence is required in `ACCEPTANCE.md` |
| Codex plugin marketplace | Unsupported | No marketplace or validator ownership promise |
| Native footer and `/status` adapter | Development preview | Not included in the standalone archive |

Source builds have an MSRV of Rust 1.85. Passing a compile job does not establish
installation compatibility.

## Get help safely

- Use the [bug form](https://github.com/snikmas/codex-usage-watch/issues/new?template=bug.yml)
  for reproducible behavior.
- Use the [compatibility form](https://github.com/snikmas/codex-usage-watch/issues/new?template=compatibility.yml)
  for Codex, schema, model, or platform changes.
- Use [private vulnerability reporting](https://github.com/snikmas/codex-usage-watch/security/advisories/new)
  for security issues. Do not open a public security issue.

Create privacy-sanitized diagnostics with:

```bash
codex-5h doctor --json
codex-5h doctor --support-bundle ./codex-usage-watch-support.json --confirm
```

Review the output before sharing. Never attach transcripts, prompts, responses,
source code, local paths, `display.json`, `state.sqlite3`, or database sidecars.

## Beta limitations

The database has no retention/compaction policy; long-running multi-window
dogfood and independent clean-machine feedback are still in progress; release
publication recovery is manual; plugin validation is not a supported user path;
and dependency security updates are reviewed manually. Re-evaluate these before
a stable release or any broader platform claim.
