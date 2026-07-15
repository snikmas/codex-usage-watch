# Native adapter policy

The native Codex footer and `/status` integration is a development preview. It
is not included in, installed by, or promised by the `0.1.0-beta.1` standalone
release. The supported beta uses the official Codex installation with explicit
user hooks and the `codex-5h status` command.

Any future adapter release must be separate from the tracker release and must:

1. publish a maintained fork ref or versioned patch against an exact upstream
   Codex commit;
2. keep accounting, SQLite, subprocesses, and transcript parsing out of the TUI
   render path;
3. read only the versioned local `display.json` projection;
4. pass focused tests and reviewed wide, medium, narrow, stale, unknown,
   malformed, and above-100% snapshots; and
5. state its supported Codex version and rebase policy.

Until those gates pass for a published source ref, adapter compatibility is
“no supported versions.” Its unfinished state cannot block the standalone beta.
