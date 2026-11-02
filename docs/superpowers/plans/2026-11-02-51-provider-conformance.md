# Provider Conformance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `executing-plans` to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** One bounded, account-free entry point for the three adapters' existing contract tests, with missing refusal/provenance checks and an honest acceptance matrix.

**Architecture:** Compose existing provider/API test binaries rather than copying protocol fixtures. Put the small cross-adapter checks in the daemon integration-test package, which already depends on all three adapters; instantiate providers without launching programs or using the shared project fixture.

**Tech Stack:** Rust integration tests, cargo-nextest, the existing Make build runner, Markdown.

**Spec:** [Issue #51](https://github.com/Androlax2/agent-cockpit/issues/51), [report 08](../../design/greenfield/08-agent-onboarding-context.md), and Theo's approved Option A in this session: fixture-only implementation and local checks, publication withheld.

## Global Constraints

- Start from main `74f27fc3e76ca6a0011d23ef28c4e7280778faec`, which contains merged #525 and #444. Use `greenfield/51-provider-conformance` in this existing isolated clone.
- No live signed-in probes, native ignored tests, installs, dependency changes, publication, issue closure or application to legacy Cockpit.
- Preserve pins: Claude Code 2.1.316, Codex 0.161.0, OpenCode 1.19.5. Constructor metadata does not establish native measurement.
- Keep #419 recognition/Claude TUI parity and #436 signed-in/pinned-version routing unmeasured; #516 persisted completion is a known failure, not unsupported.
- No shared daemon project setup while #526 remains open; no owner Git hooks/configuration, credential reads or environment mutation inside Rust tests.
- Two build jobs and two test threads; existing build-runner admission. No broad suite or repeated native runs. Missing offline prerequisites stop verification rather than triggering a fetch.
- This is a test/docs change, with no production interfaces or dependency edges added. The Makefile edit is executable host-facing configuration and must be identified in the handoff.

## Review Focus

- A fixture-only target must never opt into ignored native tests: inspect its dry-run commands and the actual nextest skipped-test report.
- An inherited OpenCode configuration must not affect the suite: remove only `OPENCODE_CONFIG_CONTENT` in each recipe's child environment; run once with a synthetic invalid value inherited by Make.
- Unsupported operations must refuse before I/O: a `SessionSockets` double panics on every connection attempt.
- An unknown version must not be promoted to tested: check every reported capability, for each concrete adapter with a bridge and no discovered version.
- Acceptance reporting must distinguish unsupported, unmeasured and failing: map each claim to the exact tests or dated native evidence; retain #516 explicitly.

## Task 1: Compose and document the fixture contract

**Files:**

- Modify `Makefile`: additive `provider-conformance` target beside `crates-test`.
- Create `crates/cockpit-daemon/tests/provider_conformance.rs`: only missing public-contract checks.
- Create `docs/PROVIDER_CONFORMANCE.md`: six-area matrix, reproduction, evidence boundaries.
- Modify `docs/design/greenfield/16-feature-coverage-map.md`: provider row links/named tests only.

**Interfaces:** Existing `Provider::capabilities`, `Provider::commands`, `Provider::invoke_command`, `Provider::deliver`, `DeliveryTarget`, and `SessionSockets`. No new product API. Existing adapter constructors receive fixed paths and bridge descriptors, never discovery or sign-in calls.

- [x] Add `an_unknown_version_never_reports_tested_capabilities`: instantiate all three bridged adapters with version `None`; for every reported capability assert `Provenance::Discovered` and `Availability::Unverified`, with provider/capability in assertion diagnostics. Assert the reported set is nonempty to avoid vacuous success.
- [x] Add `providers_without_commands_refuse_discovery` and `providers_without_commands_refuse_invocation`: real Claude/Codex adapters return exactly `ProviderError::unsupported(Capability::Commands)`, with a socket double that panics on access. Existing OpenCode server tests supply the positive authenticated cases.
- [x] Add `claude_direct_delivery_explicitly_requires_terminal_fallback`: submit and cancel return `Delivered::Unsupported` without socket access. Existing Claude input tests prove dialog/unknown-state refusal; Escape is not an interruption receipt.
- [x] Run the new integration target against unchanged production source. These are characterization checks of existing behavior; record their actual first result rather than fabricating a pre-fix RED run. A failure means investigate before changing product behavior outside the approved scope.
- [x] Add a phony `provider-conformance` target with two serial recipe lines. Both use `env -u OPENCODE_CONFIG_CONTENT CARGO_BUILD_JOBS=2 NEXTEST_TEST_THREADS=2 $(build_runner)` and `cargo nextest run --locked --offline -j 2`: first select packages `cockpit-provider-api`, `cockpit-claude`, `cockpit-codex`, `cockpit-opencode`; second select `-p cockpit-daemon --test provider_conformance`. Leave default ignored-test exclusion intact. No dependency on `greenfield-ci` or install/smoke targets.
- [x] Run `make -n provider-conformance`; confirm exactly the two intended nextest invocations. Run `OPENCODE_CONFIG_CONTENT=not-json TMPDIR=/tmp/opencode make provider-conformance` once. Record actual passes/skips; the synthetic value must not reach OpenCode launch tests. Do not inspect the real inherited value.
- [x] Write the six-area evidence matrix: receipts, resume, cancellation, commands, evidence retrieval, provenance. Link existing test binaries and the new named checks. Evidence retrieval has no trait operation and is unavailable for all three; commands are unavailable for Claude/Codex. OpenCode `Accepted` is not start/completion; Codex start/join receipts are not completion.
- [x] Include historical native evidence separately: #419 print-mode Claude versus TUI OpenCode/Codex; Codex native 0.166.0 differs from the 0.161.0 pin; #419 TLS startup failure and #436 custom-keymap preflight never reached model acceptance. Keep #516 failed and #526 outside this target. Use `Part of #51`, never a closure claim.
- [x] Add the provider-conformance coverage row. Existing ordinary workspace CI discovers the new Rust test without a workflow edit. Shared-file coordination is recorded locally because team context has no available task/member/link; no publication or host action was created to send it.

## Final verification and review

- [x] `CARGO_BUILD_JOBS=2 python3 config/bin/.local/bin/agent-build cargo clippy --locked --offline -j 2 -p cockpit-daemon --test provider_conformance -- -D warnings`
- [x] `cargo fmt --all -- --check`
- [x] `markdownlint-cli2 docs/PROVIDER_CONFORMANCE.md docs/design/greenfield/16-feature-coverage-map.md docs/superpowers/plans/2026-11-02-51-provider-conformance.md`
- [x] `lychee --offline --include-fragments=anchor-only --no-progress docs/PROVIDER_CONFORMANCE.md docs/design/greenfield/16-feature-coverage-map.md docs/superpowers/plans/2026-11-02-51-provider-conformance.md`
- [x] `typos --force-exclude --config .config/typos.toml Makefile crates/cockpit-daemon/tests/provider_conformance.rs docs/PROVIDER_CONFORMANCE.md docs/design/greenfield/16-feature-coverage-map.md docs/superpowers/plans/2026-11-02-51-provider-conformance.md`
- [x] Run the installed OpenCode `code-review medium` workflow over the full change, including new files; retain its exact outcome separately from one bounded independent OpenAI review. Fix concrete findings within scope, rechecking affected behavior. No Claude/Opus reviewer.
- [x] Run `git diff --check`, report the full diff stat and exact check outcomes, update the local chief handoff, and stop before publication. No GitHub comments, PR, push, account experiment or issue closure is authorized.

## Preparation evidence

The read-only comparison verified 154 relevant local files against main's Git blobs; no differences. #33/#47/#48 are closed. The prepared `.outgoing/provider-51-preparation.md` remains historical: its #525 retry instructions and old review-client instructions are superseded, not instructions to execute.

Fetched main and created the fresh #51 branch without tracking-config writes or history rewriting. The existing four-package baseline ran through the build runner with locked/offline dependencies, two jobs/threads and `OPENCODE_CONFIG_CONTENT` removed:

`env -u OPENCODE_CONFIG_CONTENT CARGO_BUILD_JOBS=2 NEXTEST_TEST_THREADS=2 TMPDIR=/tmp/opencode python3 config/bin/.local/bin/agent-build cargo nextest run --locked --offline -j 2 -p cockpit-provider-api -p cockpit-claude -p cockpit-codex -p cockpit-opencode`

Exit 0: `Summary [12.590s] 354 tests run: 354 passed, 30 skipped`. Those skips are not native acceptance passes. At that preparation checkpoint no new contract test had been written.

Plan checks: offline Lychee exited 0 (1 local link OK, 1 external link excluded). Initial Markdownlint exited 1 with MD032 for the file-list spacing; corrected before recheck.

## Execution evidence

The new characterization target first passed against unchanged production source:
`CARGO_BUILD_JOBS=2 NEXTEST_TEST_THREADS=2 TMPDIR=/tmp/opencode python3 config/bin/.local/bin/agent-build cargo nextest run --locked --offline -j 2 -p cockpit-daemon --test provider_conformance`
exited 0: `Summary [0.007s] 4 tests run: 4 passed, 0 skipped`. No pre-fix RED or
native measurement is claimed for these existing-behavior assertions.

`make -n provider-conformance` exited 0 and selected exactly the two intended
nextest commands. `OPENCODE_CONFIG_CONTENT=not-json TMPDIR=/tmp/opencode make provider-conformance`
then exited 0 through the task ledger runner:

- `Summary [12.535s] 354 tests run: 354 passed, 30 skipped`
- `Summary [0.006s] 4 tests run: 4 passed, 0 skipped`

Focused Clippy above exited 0 in 3.53s. Initial formatting failed on two long
statements in the new test; those were wrapped and the recheck exited 0.
Markdownlint: `Summary: 0 issues in 0 files`; offline Lychee: 22 OK, 0 errors,
3 excluded; Typos exited 0. No manifest, lockfile, provider pin or native receipt
changed. Local source is staged for complete review; publication remains withheld.

The installed medium workflow completed eight inline angles and dedup only,
without subagents or a verifier: `[]`. The separate read-only OpenAI GPT-6 Astra
review (`ses_e63966f16ffeizNeSFQzQ2VMpW`) covered all five files, staged and
unstaged, and found no critical, important or minor issues. It deemed the local
fixture-only change ready and did not rerun tests. Both outcomes and the retained
live-evidence limits are recorded under `.handoff/provider-51-*-review-20261102.*`.
The chief handoff is current; shared-file reconciliation remains a prerequisite
to future publication. No new code changes arose from review.

Final Markdownlint across seven changed source/evidence documents exited 0:
`Summary: 0 issues in 0 files`. Offline Lychee including the handoff/reviews
exited 0: 30 OK, 0 errors, 3 excluded. `git diff --check HEAD` exited 0.
Expanded Typos including local handoff files exited 2, falsely interpreting a
substring inside the preserved historical #444 commit ID as prose. The ID is retained
without a suppression; the five-file source spelling check is recorded separately.

## Publication authorization and reconciliation

After the local checkpoint, Theo explicitly authorized a separate #51 source PR
and ordinary CI. Theo then authorized squash merge with exact-head protection
after required checks and reviews pass. This supersedes the publication/merge
holds above, not the fixture-only scope. No history rewrite, release, tag or legacy
application is authorized. Source delivery is complete only when the PR is merged.

Read-only comparison against main `e91ab977b04962219efbdb7f192c871478c3d9b3`
shows two additional commits touching eight audit/security/CI-metrics files;
none touches this change or its provider code. The #192 source PR #556 at
`e37d64f701fe5f4803f8665486cd0e38e2e62235` changes the coverage map's release row
only (around line 124), while #51 inserts a provider row near line 16. Its diff
contains no Makefile edit. No conflicting hunk requires coordination or rewriting;
each PR retains its own changes. This branch retains its original base and history.

The host-executable Makefile target and the 30 skipped native tests must remain
explicit in the PR. #419/#436 acceptance remains unmeasured and #516 remains a
known failure. Ordinary CI must report actual results before delivery is complete.
