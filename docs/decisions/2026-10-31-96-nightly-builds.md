# Nightly builds stay workflow artifacts; no rolling `nightly` release

Issue #96, decided by Theo in #179 (option A).

- **Nightlies are dry runs of the release workflow.** The schedule builds, installs and
  checks `main` as `X.Y.Z-dev.N`, a version no release tag can have, and keeps the archives
  and `SHA256SUMS` as workflow artifacts for 14 days (`gh run download`). No tag, no draft,
  no release, no attestation.
- **Why:** [Releasing](../RELEASING.md) allows three tag shapes and never moves a tag, and the
  repository keeps immutable releases on; a rolling release breaks both.
- **Rejected:** a rolling `nightly` pre-release (moves its tag every night, needs immutable
  releases off); a `vX.Y.Z-nightly.YYYYMMDD` tag per night (a fourth tag shape and one release
  a day).
