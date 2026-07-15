# Beta release policy

Release `0.1.0-beta.1` is created only from an annotated tag named
`v0.1.0-beta.1` on a clean commit whose public CI is green. The tag workflow
checks that the tag and Cargo version agree, reruns formatting, lint, tests,
lifecycle and packaging gates, verifies `SHA256SUMS`, and publishes the exact
Linux x86_64 archive and crate as a GitHub prerelease.

Before pushing the tag, a maintainer must also record a clean-machine external
tester run using only the README, release candidate archive, and checksum. A
failed or missing external run blocks recommendation and tagging; it must not be
converted into a documentation exception.

Stable release requires observation-mode feedback from several real users and
does not follow automatically from a successful beta. Native adapter releases
have their own compatibility policy and version identity.
