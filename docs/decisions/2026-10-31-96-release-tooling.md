# Releases are built by a plain workflow and `tools/release.py`, with git-cliff notes, not by cargo-dist

Issue #96. Decided by the worker, as the issue and #94 asked: `dist` (report 13's proposal)
fits Cockpit's release policy worse than a short workflow of our own. The policy itself,
[Releasing](../RELEASING.md), is unchanged.

- **No cargo-dist.** `dist plan` matches the tag against the package version, so a
  pre-release (`v0.1.0-alpha.7` on a `0.1.0` workspace, no bump) fails before any build
  unless the version is stamped first, which its generated workflow does not do; editing that
  workflow means `allow-dirty = ["ci"]`, after which we maintain the YAML anyway. `dist` also
  creates and publishes the GitHub Release itself, while the policy wants a draft, the gate on
  the tagged commit, then publishing last. What it would give, archives, checksums and
  attestations, is about fifty lines of [release.yml](../../.github/workflows/release.yml).
- **`tools/release.py` is the `xtask`.** Python, like `tools/ci-jobs.py`, because it is CI
  tooling rather than a product capability, so it gets no crate in report 02's graph. It checks
  every tag against every rule, so a hand-made tag fails like a wrong one (`check-tag`: on
  `main`, channel from the gate issues, version, count, order), computes the next pre-release with every refusal of the policy
  (`next-prerelease`), stamps the version (`stamp`), packages (`package`), runs the clean-install
  check (`verify-install`) and lists gate gaps (`gate-gaps`). Tests: `tools/tests/test_release.py`,
  run first by both release workflows.
- **Stamping rewrites the internal path-dependency pins too**, in the build tree only, until
  #120 removes them; then `cargo update --workspace` and a `--locked` build.
- **Bundle layout and manifest.** `cockpit-v<version>-<platform>.tar.gz` holds
  `cockpit-v<version>-<platform>/bin/{cockpit,cockpit-daemon}`, `share/cockpit/manifest.json`,
  `LICENSE`, `README.md`, `CHANGELOG.md`; owner and timestamps are zeroed so a rebuild gives the
  same bytes (the gzip header's time too). The manifest (schema 1) names version, channel, platform, source revision,
  helpers (`{}`) and instruction bundle (`null`) until #64 and #97 ship them, and the SHA-256 of
  every file. `cockpit version --all` prints it from `<prefix>/share/cockpit/manifest.json`,
  `<prefix>` being the parent of the resolved binary's directory.
- **Platforms.** `linux-x86_64` and `linux-aarch64` built natively on `ubuntu-24.04` runners
  (the oldest image, for the glibc floor), `macos-universal` by `lipo` on `macos-26`; WSL2
  installs the Linux x86_64 archive. The x86_64 slice of the macOS archive is built and
  verified by `lipo` but not run: a named gate gap.
- **The gate runs as jobs; missing checks are named.** `gate` (`make greenfield-ci` on the
  tagged commit), `install` and `install-wsl2` (clean install on each platform: checksums,
  `cockpit version --all`, daemon start on an empty home). `GATE_CHECKS` in `tools/release.py`
  lists report 13's other checks with no job yet; `publish` refuses while one applies to the
  channel, so the release stays a draft with every asset and attestation. An alpha skips only
  the upgrade check, as the policy says.
- **git-cliff now, release-plz later.** `cliff.toml` groups conventional subjects by type and
  puts the earlier, unconventional ones under "Other" rather than dropping them. release-plz
  joins with the first stable release PR (#101); before it, `next-prerelease` refuses once a
  stable tag exists instead of guessing the release PR's version. Alpha notes append
  `docs/KNOWN-GAPS.md` (#97); the workflow refuses an alpha without it.
- **Only tags are attested.** Dry runs (pull requests, nightlies, dispatches) never get
  `id-token` or `attestations: write`, so no unreleased build verifies as this repository's.
- **Pre-release tags come from a GitHub App token** (`RELEASE_APP_CLIENT_ID`,
  `RELEASE_APP_PRIVATE_KEY`), which Theo creates; the dry run needs neither.
- **Nightly builds are dry runs.** The schedule builds and installs `main` as
  `X.Y.Z-dev.N` and keeps the archives as workflow artifacts for 14 days. A rolling `nightly`
  pre-release would move a tag: [decided against](2026-10-31-96-nightly-builds.md) in #179.
- **Rejected:** cargo-dist for the build half only (two release owners for one GitHub
  Release); nfpm and minisign now (packages and signatures belong to #64 and #97, and adding
  them later changes no tag or asset name); a Rust `xtask` crate (outside report 02's graph).
