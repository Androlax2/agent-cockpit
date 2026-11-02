# Managed engine installation implementation plan

> Use the executing-plans skill in this lane, with one fresh full-delta review per PR.

**Goal:** Install the releases in `crates/cockpit-sandbox/engine-pins.json` into
Cockpit-owned paths, and use those paths for every greenfield launch.

**Architecture:** `cockpit-sandbox` owns pin selection, verification, extraction and
activation. The owner CLI exposes installation before daemon startup; the daemon resolves
the same immutable pair of pinned engines. Readiness and onboarding name that command.

**Spec:** Issue #470 and its comments, reports 11 and 13, and the Alpha 1 lane brief.

**Tech stack:** Rust, the existing pin JSON, bounded HTTPS download through curl.
Theo approved tar/flate2, existing sha2/tempfile versions and the CLI edge on 2026-11-02.
The standalone CLI disables sandbox persistence; daemon/session callers own database access.

## Constraints

- One issue and one PR on `greenfield/470-managed-sandbox`; no host application.
- Only the explicitly approved dependency/manifest changes; no fork release/tag creation.
- Preserve greywall's versioned directory, greywatch, LICENSE, NOTICE and LICENSES;
  preserve greyproxy's flat binary and LICENSE.
- Verify SHA-256 before extraction or execution, then validate each exact commit.
- Never resolve release engines from PATH, legacy packages, or an unpinned override.
- Existing installations and running sessions survive a failed upgrade.
- Old installed versions stay until cutover/retention owns removal.
- Concurrent-session acceptance awaits a published Greyproxy build containing fork PR #8.
- Use isolated homes and fake downloads/engines, focused tests with two build/test workers.
- Release workflow edits belong to the release lane; CI supplies platform evidence.

## Task 1: Installer

Files: new `crates/cockpit-sandbox/src/install.rs` and its tests; crate exports and approved
manifest changes; retain the existing single-source pin file.

- [x] Add failing fixtures for both real archive shapes, mismatch, truncated archive,
  traversal/link entries, missing licences, wrong commit, unsupported target and reruns.
- [x] Parse and validate pins; select a supported target before creating any installation.
- [x] Download with bounded size/time, verify both archives, extract regular files
  and directories inside private staging, preserve the published `greywatch -> greywall`
  alias, reject other links, and validate binaries and licence material.
- [x] Atomically activate the complete versioned pair; clean staging on all error paths.
- [x] Prove successful reruns and concurrent installs cannot corrupt a valid installation.

## Task 2: Product entry points

Files: `crates/cockpit-cli/src/main.rs`, installer adapter and CLI tests;
`crates/cockpit-daemon/src/serve.rs`, real-daemon fixtures; readiness and onboarding tests.

- [x] Add/run a CLI regression: install is currently rejected as unknown arguments.
- [x] Add the approved offline installation entry point, with typed failure rendering.
- [x] Resolve the same owned engine paths in daemon startup; refuse PATH substitutes.
- [x] Permit explicit local binaries only in a development profile of a debug build.
- [x] Show the installation command in missing-engine readiness.
- [x] Verify the readiness action reaches onboarding through its real entry point.
- [x] Report sessions still carrying older engine evidence as needing restart.
- [x] Run real CLI/daemon fixtures proving installation resolution, guidance and upgrade
  behavior; preserve exact-commit validation and agent authorization boundaries.

## Task 3: Evidence and review

- [x] Provide a bounded clean-runner install/doctor probe using the published assets,
  coordinate CI integration with the release lane, and keep unmeasured claims open.
- [x] Add the legacy-install removal step to #66 without touching any legacy install.
- [x] Update report 16 evidence and a dated #470 decision record.
- [x] Run affected nextest suites, format, Clippy and crate graph checks once source settles.
- [x] Native review is unavailable in this harness; use the authorized fresh-agent fallback
  against the full delta. Fix substantive findings and report exact verification outcomes.
- [ ] Commit and normally push this branch, create the PR and arm squash auto-merge only
  after review/checks. Use `Part of #470` unless every acceptance criterion is proven.
- [ ] After merge, start #495 from updated origin/main on its own branch.

## Review focus

- Checksum failure on the second engine must not expose a half-installed pair (task 1).
- Archives must not escape staging through names, links or duplicate members (task 1).
- A failed upgrade must leave older engines and their running sessions intact (tasks 1–2).
- An existing legacy executable must never satisfy greenfield readiness (task 2).
- A packaged/release build must refuse local-development overrides (task 2).
