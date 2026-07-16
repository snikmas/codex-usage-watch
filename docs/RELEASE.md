# Beta release policy

Release `0.1.0-beta.1` is created only from an annotated tag named
`v0.1.0-beta.1` on a clean commit whose public CI is green. The tag workflow
checks that the tag and Cargo version agree, reruns formatting, lint, tests,
lifecycle and packaging gates, verifies `SHA256SUMS`, and publishes the exact
Linux x86_64 archive and crate as a GitHub prerelease.

Every archive includes `BUILD-INFO.json` with the source revision, target,
toolchain versions, source timestamp, dirty-state flag, and `Cargo.lock` digest,
plus an SPDX 2.3 `SBOM.spdx.json` generated from locked Cargo metadata. The
release gate requires `source_dirty` to be `false` and validates representative
root and runtime packages in the SBOM. Signatures remain deferred until the
project has a repeatable keyless or maintained-key verification flow.

Before pushing the tag, a maintainer must also record a clean-machine external
tester run using only the README, release candidate archive, and checksum. A
failed or missing external run blocks recommendation and tagging; it must not be
converted into a documentation exception.

Stable release requires observation-mode feedback from several real users and
does not follow automatically from a successful beta. Native adapter releases
have their own compatibility policy and version identity.

## Maintainer release checklist

- [ ] Stage 12 state-permission and extracted artifact/crate privacy tests pass.
- [ ] The candidate worktree is clean and `BUILD-INFO.json` records the frozen
  SHA with `source_dirty: false`.
- [ ] Every required public CI check is green and `main` protection is active.
- [ ] Sanitized real hook trust, elapsed-window dogfood, and one independent clean
  Linux x86_64 lifecycle are recorded in `docs/ACCEPTANCE.md`.
- [ ] The repository's complete release gate passes from the frozen commit.
- [ ] The annotated tag matches Cargo/plugin/docs versions.
- [ ] The workflow-published archive and checksum are downloaded into a clean
  directory and independently reverified before public recommendation.
