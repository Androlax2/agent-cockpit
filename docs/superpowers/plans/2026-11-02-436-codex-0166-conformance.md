# Codex 0.166.0 capability revalidation implementation plan

> **For agentic workers:** Use `executing-plans` to implement this plan inline, task by task. Independent full-delta review remains a separate delivery gate.

**Goal:** Validate the five advertised Codex capabilities on native 0.166.0 before promoting that version to the production capability pin, then obtain separately approved signed-in approval-routing evidence for #436.

**Architecture:** Reuse the adapter, real session launcher, recording proxy and local Responses fixture. Replace the old native suite's external placeholder-key calls with isolated loopback responses, preserve refusal guards, and promote the pin only after exact-version native observations pass.

**Tech Stack:** Rust integration tests, existing Codex 0.166.0, private tmux, loopback HTTP, existing Cargo dependencies.

**Spec:** Issue #436 and Theo's explicit selection of Option B in this session on 2026-11-02. Existing capability contract: `crates/cockpit-codex/src/lib.rs::capabilities`.

## Global constraints

- Target **0.166.0**; supported capabilities stay Resume, Events, Submit, Cancel and Outcome.
- Older/different/unknown versions remain unverified after promotion, not unsupported.
- #51 is completed. Do not reopen it or rerun its composed suite.
- Historical 0.161.0 payloads and receipts retain their original provenance.
- No dependencies, manifest edits, downloads, default-branch writes or legacy application.
- Native execution, compilation needed on the host and account use require their normal approvals. User selection of the version is not account-use approval.
- Linux/WSL2 Alpha 1; macOS acceptance deferred. Candidate/onboarding remains coordinated with its existing owners.

## Review focus

- Inherited credentials, integrations, proxies and project configuration must not enter isolated native checks.
- Placeholder credentials must not cause calls to real provider endpoints.
- A new version label must not relabel old captured payloads or claim unobserved capabilities.
- Persistent TUI ownership, delivery disconnection and request/turn identity must remain independently checked.
- Cleanup must stop child process groups and the private tmux server on success, refusal and failure.

## Task 1: Make native capability checks isolated and deterministic

**Files:**

- Modify `crates/cockpit-codex/tests/conformance.rs`.
- Extend `crates/cockpit-codex/tests/common/approval_model.rs` only for the additional local-response scenarios needed here.
- Reuse `tests/common/{approval_preflight,bounded_command,recording_proxy}.rs`.
- Adjust affected callers in `tests/common/approval_probe.rs` only if a shared fixture interface changes.

**Interfaces:** Preserve existing approval fixture behavior. Add explicitly selected local successful and gated-error responses to the existing `Model`; root-turn gating must exclude title helpers. Conformance children receive cleared environments, scratch HOME/XDG/provider directories, explicit local model routing and bounded cleanup.

- [x] Add ordinary child-process regressions proving inherited auth/endpoint/config markers do not reach the fake native executable, and hung helpers are terminated within their deadline.
- [x] Run the new regressions against the previous setup; record the actual failure before implementing the isolation correction.
- [x] Replace external placeholder-key behavior in `fresh_home`/`LiveServer` with the loopback model. Preserve real `server_arguments`, hook subscription/trust and real adapter entry points.
- [x] Make `codex_output`, server startup and tmux commands use the existing bounded-command helper and scratch environment. Keep normal cleanup and failure cleanup explicit.
- [x] Reuse the preflight checks for effective local routing, integrations, rules and keymaps before starting native turns. Unknown configuration remains a refusal.
- [x] Keep existing native assertions for flags/features, trusted hooks, prompt reports, submit/steer/cancel, failed outcomes and session-launcher teardown. Drive failed outcomes with a local terminal HTTP error; gate responses for steer/cancel determinism.
- [x] Add native coverage that completes a local successful turn, stops the TUI, resumes the same conversation through `Provider::launch`, and observes the same conversation identity and a new prompt report. Verify successful outcome reporting as well as the existing failed-outcome path.
- [x] Run focused ordinary crate tests with two build jobs/test threads. No broad suite.

## Task 2: Establish version-specific evidence, then promote the pin

**Files:**

- Modify `crates/cockpit-codex/{src/lib.rs,tests/capabilities.rs,tests/common/mod.rs,tests/hooks.rs,README.md}`.
- Add `crates/cockpit-codex/tests/fixtures/codex-0.166.0/README.md` and exact upstream hook schemas as needed for schema validation.
- Modify `docs/PROVIDER_CONFORMANCE.md` and the existing #436 coverage-map row.
- Add a decision record (removed in #1029; the policy lives in the crate and fixture READMEs).

**Interfaces:** `CONFORMANCE_VERSION` becomes `"0.166.0"` only after evidence. Existing 0.161.0 fixture access remains explicitly historical; the pin must not silently redirect reads to copied-and-relabelled payloads.

- [x] Compare subscribed input schemas against exact upstream `rust-v0.166.0`; record source commit and hashes. Verify every trusted-hook hash against the native server. Keep authored schema cases distinguished from native captures.
- [x] Add capability regressions asserting all five capabilities are tested on 0.166.0, while 0.161.0, another version and unknown remain unverified. Observe their failure before promotion.
- [x] During native evidence collection, assert the candidate executable is exactly 0.166.0 independently of the still-old production pin; keep the new production-provenance regression red until promotion. Do not spoof version output or weaken the exact-version check.
- [x] Request one bounded account-free host/native verification command naming the exact approved executable and checks. Include the updated capability suite and existing isolated routing/refusal checks; exclude the signed-in test.
- [x] If any observation fails, fix only the verified cause and rerun the affected check. Do not promote the pin from partial evidence.
- [x] Promote `CONFORMANCE_VERSION`, update current documentation/coverage, and retain historical evidence verbatim. Record exact source, executable identity and native results.
- [x] Run final focused crate tests, Clippy, formatting and affected docs/spelling/link checks. Check daemon/CLI capability consumers as required by the actual diff.

## Task 3: Review, live acceptance and source delivery

**Files:** Current #436 decision/evidence documentation and `.handoff/provider-live-acceptance-20261102.md` / `.handoff/chief-remaining-work-20261102.md`.

- [x] Run installed `code-review medium` on the full branch delta and one fresh independent full-delta review. Resolve findings and recheck affected paths.
- [x] Request signed-in preflight/live execution only when the exact account/model and effective configuration are approved. Initial inventory alone granted no usage permission; later host approvals are recorded below.
- [x] Use the existing signed-in runner with all guards intact: initial READY turn, one delivered approval request, possible title usage, cancellation only, request/connection/turn evidence, deadlines and cleanup. One approved attempt; stop at refusal.
- [x] Publish a scoped PR with `Part of #436` until actual acceptance is proven. Run ordinary CI and merge only after required checks pass, with `gh pr merge --squash --match-head-commit <reviewed-head>` under the applicable authorization.
- [ ] Close #436 only after the live criterion and promoted-version evidence are both satisfied. No release, tag or legacy apply.
- [ ] Resume #419 using the docs owner's real candidate/onboarding sessions. Three provider brief-recognition observations and Claude TUI trust still need separate readiness/account approval; this Codex plan does not complete them.

## Commands and evidence recording

Focused ordinary commands use `agent-build` when available, `CARGO_BUILD_JOBS=2`, `NEXTEST_TEST_THREADS=2`, `--locked --offline`, and package `cockpit-codex`. Native commands are prepared only after the isolation changes are reviewed and the explicit binary path is known. Record exact executed commands/results here; skipped or pending checks are not passes.

## Status

- [x] Theo selected Option B: move the capability pin to 0.166.0, with expanded revalidation.
- [x] Dedicated clone preserved; new branch `greenfield/436-codex-0166-conformance` starts at main `6a35822ab452fe42aa092a109f58b30f181e7975`.
- [x] Host version-only receipt confirms installed native launcher reports `codex-cli 0.166.0`.
- [x] Theo reviewed this execution plan and approved inline implementation, focused compilation and verification.
- [x] Capability revalidation implementation and focused verification.
- [x] Signed-in #436 acceptance, source CI and delivery (PR #576; receipt `edeea0394e55`).

## Subsequent source delivery and live acceptance

PR #576 merged as `8d916180cce0a664a94553e7ac735442be51c7ff` after all five
required checks passed, using exact-head-protected squash merge. The separately
approved existing-home attempt `9ffb83dbb87e` then refused inherited rules before
a turn (exit 100). Theo selected a private validation home and approved official
sign-in (`fa4200cef0fd`, exit 0), without copying credentials or altering the
existing home. A new exact host approval `edeea0394e55` passed the live criterion:
1 passed, 19 skipped, exit 0. The TUI returned `cancel` and the matching turn ended
interrupted. The [acceptance receipt](../../acceptance/436-codex-approval-routing.md)
records the command, configuration, preserved failures and platform boundaries.
Issue #419 remains a separate, unfinished acceptance task.

## Execution evidence

All host requests below ran through `ask-to-run`, with explicit approval. Failed
receipts stay failed; follow-ups do not replace them.

| Receipt | Result |
| --- | --- |
| `b41326724024` → `bea27abb69ce` | Environment isolation RED: 0 passed / 1 failed / 8 skipped, exit 100. GREEN: 3 passed / 8 skipped, exit 0 |
| `63577d0a5d94` → `16f0d50a2176` | Local-model scenarios RED: 0 passed / 2 failed / 16 skipped, exit 100. GREEN: 13 passed / 19 skipped, exit 0 |
| `2618146fdd94` | Native run: 16 passed / 1 failed / 14 skipped, one test not run, exit 100; resume rejected all-null effective keymap |
| `e573b4016fd2` | Targeted diagnostic: 0 passed / 1 failed / 13 skipped, exit 100; effective keymap contains only object/null defaults |
| `b808b7d58d71` → `2727d19ab396` | Default-map and pin RED: 0 passed / 3 failed / 25 skipped. Keymap GREEN: 2 passed / 18 skipped; native refusal/resume/terminal launch: 3 passed / 31 skipped, exit 0 |
| `9ade3c16d92d` | Ordinary Codex: 136 passed / 20 skipped; daemon consumers: 10 passed / 0 skipped. Clippy duplicate module import failed, exit 101; corrected by reusing the root import |
| `c2e57f1cba60` | Both focused Clippy checks passed; approval fixtures: 10 passed / 10 skipped. Native: 9 passed / 1 failed / 16 skipped, eight unrun, exit 100; interleaved shell report exposed |
| `d13bcb90ec65` → `7e8ffee9e9a6` | Review RED: 0 passed / 2 failed / 14 skipped. GREEN: 2 passed / 14 skipped; ordinary Codex: 137 passed / 21 skipped. Clippy constant assertion failed, exit 101; corrected to check the actual required proc filesystem |
| `e2dc023b35a9` | Final Codex Clippy and Rustdoc passed; affected ordinary/native checks: **35 passed / 1 skipped**, exit 0; only the signed-in test excluded |

Final executable commands (host compilation used `CARGO_BUILD_JOBS=2` and
`agent-build`; ordinary nextest used two test threads, native checks one):

```sh
cargo nextest run --locked --offline -p cockpit-codex -j 2
cargo nextest run --locked --offline -p cockpit-daemon --test providers --test hook_bridge -j 2
cargo clippy --locked --offline -p cockpit-codex --all-targets -- -D warnings
cargo clippy --locked --offline -p cockpit-daemon --test providers --test hook_bridge -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --offline -p cockpit-codex --no-deps --document-private-items
COCKPIT_CODEX_PROGRAM=/home/theo/.local/npm/bin/codex \
  cargo nextest run --locked --offline -p cockpit-codex \
  --test conformance --test approval_routing --run-ignored all \
  -E 'not test(=a_signed_in_delivered_turn_reaches_the_real_tui)' -j 1
```

Formatting initially failed on a line wrap and Markdown on an unescaped leading
issue number in a handoff; both corrected. Subsequent `cargo fmt --all -- --check`,
focused markdownlint, lychee (30 total, 28 OK, two excluded, zero errors), typos
and `git diff --check` passed. No #51 composed-suite rerun.

## Review and rulings

- Installed OpenCode `code-review medium`: eight inline angles, dedup only, no
  verifier. Found incomplete report publication in the new resume test.
- Independent full-delta reviewer `ses_e62ed5c66ffeCL107dmdWfcmiY`: no critical or
  minor findings; one Important failure-cleanup finding. No nested agents or tests.
- Repeated installed medium review after the substantial containment fix: all
  eight inline angles on the full corrected delta, dedup-only result `[]`.
- Both findings reproduced before correction. The recorder exposes complete
  records only; direct native servers use the launcher's report serialization.
  Terminal fixtures retain a pane group leader, gate child start until its ID is
  recorded, escalate TERM/KILL, verify no running group member via Linux `/proc`,
  and explicitly stop private tmux. Failure preserves the session directory.
- Ruling: reuse the already isolated clone; no additional worktree. Cost if wrong:
  cross-lane interference, bounded by the dedicated branch and scoped file staging.
- Ruling: reuse identical upstream schemas with original payload provenance,
  rather than relabel copies. Cost if wrong: overstated compatibility; separate
  native reports and exact-version checks remain required.
- Ruling: accept object/null default maps only, keeping all concrete bindings and
  arrays refused. Cost if wrong: unsafe cancellation; ordinary and real-native
  remapping/unbinding refusal tests both passed.
- Ruling: native trust-hash comparison is stronger than inferring acceptance from
  upstream normalization code alone; each subscribed hook's current hash and trust
  status were checked on 0.166.0. Cost if wrong: future drift; the test stays pinned.
- Ruling: independent review's declined signed-in/#419/platform claims remain
  explicit gaps. Linux native terminal cleanup needs `/proc`; macOS is deferred,
  and these observations are not WSL2 acceptance. Cost if wrong: overstated release
  readiness; the acceptance issues remain open.
