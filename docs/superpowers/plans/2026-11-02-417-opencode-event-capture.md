# OpenCode native plugin event capture implementation plan

> **For agentic workers:** Use the executing-plans skill inline; obtain one fresh independent full-delta review before the PR.

**Goal:** Prove that OpenCode 1.19.5 loads Cockpit's plugin through the real launch configuration and emits the tool, permission and aborted-turn events the adapter consumes.

**Architecture:** A Rust integration test uses `Provider::launch` and the built bridge.
A Python standard-library fixture supplies a loopback model, recording bridge and fake
Cockpit command while the exact native harness supplies every captured event. Captured
fixtures are redacted and replayed through the adapter separately from schema-derived cases.

**Tech Stack:** Existing Rust/serde_json/tempfile and Python 3; the approved standalone OpenCode 1.19.5 binary. No dependencies or manifests change.

**Spec:** [Issue #417](https://github.com/Androlax2/agent-cockpit/issues/417), with its current comments, and Theo's provider-validation brief and explicit download approval.

## Constraints and review focus

- The approved release archive is 79,651,896 bytes, SHA-256
  `588310a336fa3b9df36b3f0225b26f679d453efaad0cf3d66c3651ef3ec1647a`.
  Extracted executable SHA-256 is
  `7df4e4136301f16589b0f30bf91a61380fca5f8ef22dd36570f5ebe1d4ec802c`;
  isolated `--version` returned `1.19.5`. Installed 2.1.4 is not this proof.
- Fresh homes and cleared child environments; disable project configuration, model catalog
  fetching, updates, file watchers, LSP and sharing. No real auth or account requests.
- Use the bundled compatible-provider SDK and a local model. The temporary config directory
  is read-only: 1.19.5's `Npm.install` then returns without installing dependencies. Assert
  that no package installation occurred. Restore fixture permissions for cleanup.
- Read only a fixture file. Request a harmless command but reject its permission through the
  authenticated owner API; never send an acceptance. Abort a separate held model turn.
- Bound process lifetimes, HTTP reads and capture sizes; clean up fixture descendants on
  failure. Never run the host's Cockpit command or reuse its server/password/session.
- Do not label schema-written or unobserved events captured. Compare the captured shapes
  and normalized reports; preserve source-version and redaction provenance.

## Task 1: Native capture through the launch and bridge

**Files:** `crates/cockpit-opencode/tests/plugin_conformance.rs`, helper scripts under
`tests/common/`, and only the plugin/adapter code if an observed mismatch requires it.

- [x] Add a failing ignored exact-version integration test for a tool completion, permission
  request/refusal and aborted turn through the actual plugin and built Rust bridge.
- [x] Implement a bounded fixture using the real launch words and inline configuration;
  the recorder forwards each native event unchanged to the built bridge.
- [x] Exercise the local model and capture helpers through focused tests before claiming
  native proof. Assert the fixture's brief reaches model requests, plugin events name the
  native session, permission is refused, and abort produces no failed/completed outcome.
- [x] Run the pinned harness in isolation; investigate actual failures rather than weakening
  assertions or using the installed 2.1.4 binary.

## Task 2: Replay, provenance and reviewed PR

**Files:** `tests/fixtures/opencode-1.19.5/`, replay tests, adapter README, report 16's
provider row, and `docs/decisions/2026-11-02-417-opencode-event-capture.md`.

- [x] Redact only volatile IDs, paths, times and model-fixture metadata; retain field shapes
  and relationships. Replay captured events through `events::normalize` and the built bridge.
- [x] Document the exact capture command, binary/archive digests, observed runtime and
  outcomes. Keep remaining schema-derived fixtures explicitly labelled.
- [x] Run focused tests while iterating, then the affected crate, formatting, Clippy and
  script/docs checks once the source settles. Let CI prove supported platforms.
- [ ] Obtain the required fresh review, commit and normal-push this issue's branch, open
  one PR, and arm squash auto-merge only after review and focused checks pass.

## Execution record

- Branch `greenfield/417-opencode-event-capture` starts at merged PR #512 (`a3b0906`).
- Baseline `CARGO_BUILD_JOBS=2 NEXTEST_TEST_THREADS=2 cargo nextest run --locked -p cockpit-opencode -j 2`:
  exit 0, 101 passed and five skipped.
- Theo approved only the pinned download/extraction for isolated work. It is under
  `.outgoing/opencode-1.19.5-7mi5obxc/`, with `provenance.json`; no installation or PATH
  change was made and the version check used an empty home with no account access.
- Native command: `CARGO_BUILD_JOBS=2 COCKPIT_OPENCODE_PROGRAM=$PWD/.outgoing/opencode-1.19.5-7mi5obxc/opencode COCKPIT_OPENCODE_CAPTURE_DIR=$PWD/.outgoing/capture-417-provenance cargo nextest run --locked -p cockpit-opencode --test plugin_conformance -j 2 --run-ignored only --no-capture`:
  exit 0, one passed, two helper tests excluded. OpenCode 1.19.5/Bun 1.4.4; 19 native
  events and matching bridge reports, four local model requests, no account use.
- Ordinary affected-crate final command: `CARGO_BUILD_JOBS=2 NEXTEST_TEST_THREADS=2 cargo nextest run --locked -p cockpit-opencode -j 2`:
  exit 0, 105 passed, six native prerequisites skipped; log `/tmp/cockpit-417-final-tests.log`.
- `CARGO_BUILD_JOBS=2 cargo clippy --locked -p cockpit-opencode --all-targets -j 2 -- -D warnings`
  and `cargo fmt --all -- --check`: exit 0. Focused Ruff check/format, Markdown lint and
  offline Lychee checks: exit 0. The full workspace suite was not run locally; CI owns
  the supported-platform gate.
- Iteration failures were test-fixture assumptions: the native read permission uses a
  worktree-relative path, and a rejected command ends idle without an assistant stop
  event. Exact 1.19.5 `read.ts` and `processor.ts` explain both. The first capture also
  exposed a writer/count race; the final capture waits for terminal idle and snapshots
  matched records. Ruff formatting/import failures were corrected. Initial helper and
  replay tests failed before their files/fixtures existed, then passed with real captures.
- Native `/code-review` is unavailable in this toolset. Use the user's authorized fresh
  independent agent review for the complete branch delta; review remains pending here.
