# Experimental beta release policy

`0.1.0-beta.1` is an experimental beta only for Ubuntu 25.10 x86_64, installed
from the checksummed standalone archive and exercised with Codex CLI 0.144.4.
The Rust target in the filename is build metadata, not a general platform claim.

The annotated tag `v0.1.0-beta.1` may be created only from a clean commit whose
required public CI is green. The tag workflow reruns formatting, lint, tests,
lifecycle and privacy gates, builds the archive and crate, verifies
`SHA256SUMS`, and publishes a GitHub prerelease.

Every archive contains `BUILD-INFO.json`, an SPDX 2.3 `SBOM.spdx.json`, the
locked binary, documentation, and lifecycle helpers. These records are useful
provenance, not cryptographic signatures.

## Required before tagging

- The full source and exact-artifact release gate passes from the frozen commit.
- Public CI is green and protected `main` points at that commit.
- Private vulnerability reporting is enabled and visible to a non-maintainer.
- The real Codex `SessionStart`, `UserPromptSubmit`, and `Stop` hooks pass after
  interactive trust approval, including changed-definition re-review.
- The candidate archive is installed and uninstalled by following only public
  onboarding, and sanitized evidence is recorded in `ACCEPTANCE.md`.
- Public text consistently says experimental beta, local estimate, and the exact
  tested environment.

## Required after publication, before recommendation

Download the workflow-published archive and `SHA256SUMS` into a new empty
directory. Verify the checksum, install it, exercise all three trusted hooks,
run `status` and `doctor`, and uninstall it. Compare its build identity and
checksum with the frozen candidate evidence. Any mismatch blocks recommendation.

Publication recovery is manual for this first beta. If the tag workflow partly
fails, stop, inspect the release and tag, remove or correct inconsistent public
artifacts deliberately, and rerun the entire verification. Do not describe the
workflow as retry-safe.

## Deferred beta follow-up

Several naturally elapsed five-hour windows, independent clean-machine testing,
database retention/compaction, broader portability, retry-safe publication,
plugin-validator ownership, and automated dependency security-update handling
remain open. Revisit them during beta and before any stable or broader support
claim.

Stable release requires sustained dogfood and real-user feedback; it does not
follow automatically from a successful beta.
