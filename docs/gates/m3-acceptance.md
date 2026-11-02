# Alpha 1 acceptance preparation and evidence

Status: **prepared, not accepted**. No Theo run is recorded. Issues [#137](https://github.com/Androlax2/agent-cockpit/issues/137) and [#52](https://github.com/Androlax2/agent-cockpit/issues/52) remain open.
The `m3` filenames preserve the issue links; Alpha 1 is the current release name.

## How to use the preparation

The [guide](../acceptance/m3.md) is in ask-to-guide step format. A facilitator first resolves the prerequisites below and asks for readiness. Do not launch the guide overnight or treat its questions as answered.

The [offline runner](../../tools/acceptance/alpha1.py) accepts only an explicit candidate directory and an existing short scratch parent:

```sh
python3 tools/acceptance/alpha1.py --bin-dir target/debug --parent /tmp/opencode
```

This example requires a previously built greenfield CLI in `target/debug`; the script never builds or downloads it. For an archive, substitute its reviewed bin directory and record the archive checksum separately. Outside this sandbox choose an existing disposable parent appropriate to the host, keeping the path short for Unix sockets.

The runner creates a new owner-only home for every run, passes an allowlisted environment and explicit `--profile next`, then runs only `version --all`, `config show`, `init` and `doctor --project` on its empty scratch project. It prints the path to `evidence.json`; stdout/stderr and actual exit codes remain beside it. Commands have a 20-second deadline. Exit 0 means these **offline expectations** matched; doctor is expected to return 1 with config available, daemon missing and host unknown. It does not mean readiness or acceptance. Unexpected exits fail the rehearsal and preserve evidence.

No provider, daemon, installer, desktop command or service manager is launched. No environment credential, agent token, desktop bus, proxy setting or owner HOME is inherited. This wrapper is not an OS sandbox: run only a reviewed candidate, with normal approval when execution is on the host. The historical [M1 runner](../../tools/acceptance/run.sh) deliberately retains the real login/desktop and is unsuitable for this clean-home walk.

## Prerequisites and morning handoff

| Prerequisite | Owner / exact remaining evidence |
| --- | --- |
| Named review of this preparation | Completed on PR #530's full six-file delta through `83193277ecf486c52252bbdc3ab72132525c445b`; the sole finding was the stale unavailable-review claim, corrected here. Independent review remains separate; see the receipt below. |
| Candidate installation | Release lane #191/#192/#97: final candidate revision, archive digest, published install instructions, platform install/doctor receipts. No publication or installation was performed here. |
| Sandbox/network | Sandbox lane #495/#433 and macOS #527: clean-home managed proxy lifecycle, endpoint collision refusal, real enforcement and four working consecutive launches. A profile isolates paths, not fixed TCP ports; legacy API port 43080 is documented in `packages/greyproxy/restart.sh`. Do not stop/reconfigure legacy or silently reuse it. |
| Providers | Provider lane #51/#419/#436: conformance/provenance matrix, real model sees brief/skills, exact delivered-turn approval destination. Explicit consent and clean-environment sign-in precede account usage. |
| Requirement clarification | #137 still says “one ACP agent”; merged third provider is OpenCode's HTTP/plugin adapter. #479 owns missing total-context token measurement. Runtime logging override lacks a CLI route at the inspected revision. These are open, not silently waived. |
| Human observations | #137: Theo; #98: a volunteer with no prior context. Neither has a recorded Alpha walk. #98's separate [walkthrough record](../acceptance/98-user-readme.md) stays with the docs owner. |

Morning question, **only after those dependencies are ready or explicitly deferred on #52**:

> Are you ready for a 45–60 minute isolated Alpha 1 walk covering first run, three providers and diagnostics, plus 15–30 minutes if fresh sign-in is needed?

If dependencies remain open, Chief should present their specific missing receipt instead of opening a guide. The [#97 coordination handoff](https://github.com/Androlax2/agent-cockpit/issues/97#issuecomment-6569328690) requests candidate and platform receipts from release/docs owners. No files in those lanes are edited here.

## Evidence inventory at the inspected revision

Snapshot: `ea5458970720cf24dd807c21a1cdb808085ed5a4`, 2026-11-02. Source references below are present at that revision; a merged implementation is not live acceptance.
The expanded [#52 inventory](https://github.com/Androlax2/agent-cockpit/issues/52#issuecomment-6569435976) separates each provider's native evidence and unsupported capabilities.

Baseline [CI run 41388684998](https://github.com/Androlax2/agent-cockpit/actions/runs/41388684998) passed Linux greenfield, macOS, WSL2 and docs. Linux runs the greenfield workspace gate. macOS covers startup/processes/resources/agent identity/TUI; WSL2 covers startup/resources. Neither platform job is a six-cell onboarding or public-install proof. Ignored native tests are not established by a green workspace job.

Codex's additional native receipt is #512 (`a3b09065`), [approval-routing decision](../decisions/2026-11-02-436-approval-routing-evidence.md): a real 0.166.0 TUI with a local fake model received and cancelled delivered-start/steer approvals after the delivery connection closed. The 0.161.0 capability pin was not changed. The signed-in attempt exited 101 at the custom-keymap guard before any model turn, so #436 stays open.

| Alpha requirement | Merged implementation / exact proving fixture or receipt | Remaining gap |
| --- | --- | --- |
| Conformance ×3 (#51) | Provider adapters and delivery wiring: #469 (`9fa26814`), #490 (`9536d7a6`), #489 (`14e318a6`). Each adapter has `tests/conformance.rs`; daemon `tests/codex_messaging.rs` and `tests/opencode_messaging.rs` exercise delivery/cancel. #515 (`e05bf5d3`) records native OpenCode 1.19.5/Bun 1.4.4 plugin load: 19 events/bridge reports, four loopback model calls, no account use. | Provider lane owns complete supported/unsupported matrix and #436 approval routing. Native plugin capture does not prove signed-in model understanding or persistent completion; #516 names the latter gap. |
| Clean-home onboarding (#50/#100) | #447 (`7d9fac39`), #486 (`33e2bea0`), #491 (`cd7ca73f`). CLI `tests/doctor.rs::without_a_daemon_the_daemon_is_the_missing_prerequisite`; `tests/onboarding.rs` drives the actual CLI against scripted daemon facts. Readiness `tests/report.rs::every_requirement_that_is_not_available_names_its_next_action`; `tests/docker.rs` checks engine identity/refusal through fake Docker. | No complete Linux/macOS/WSL2 × Docker-present/absent real onboarding receipts. Two-OS-user isolation, real provider sign-in and a permitted check remain separate observations. Offline rehearsal proves none of these. |
| Context measured (#49/#419) | #422 (`35aeefff`) plus #428 (`e2aaf926`); #469 brief/MCP delivery, #481 (`94d2f006`) native skill registration. Agent-context `tests/brief.rs::the_budget_separates_what_is_always_loaded_from_what_is_read_on_demand`; daemon `tests/agent_brief.rs`; CLI `src/instructions.rs` prints brief/catalog/manual byte counts. | #419 real model receives instructions; #479 harness total tokens. Do not convert bytes to tokens or claim a full context comparison. |
| Coexistence/install (#134/#97) | #449 (`e1f0e50e`) selects next profile for paths, sockets, unit and tmux; CLI `tests/profiles.rs` covers selection/refusal. #493 archive fix has Linux/macOS/WSL2 receipts recorded on #192. | Public-archive onboarding under #97 and occupied TCP endpoints under #495. Source build and development-archive install do not establish a published installation. |
| Sandbox (#470/#495/#433) | #511 (`2553ebff`) installs verified engines in profile-owned paths; #430 (`f9fd6836`) routes launches through sandbox. Matching binaries and plan fixtures are implemented. | Proxy lifecycle and live enforcement on each platform remain owned by sandbox lane; native macOS CA handling #527. Never infer enforcement from requested rules. |
| Release tooling (#191/#192) | #513 (`570fe35`) history-preserving proposals; #518 (`5820574`) generated-doc correction. #192 records actual App-owned proposal and same-base retry receipts. | #192 version-source clarification; #191 candidate gates in PR #523 were unmerged at inventory time. No tag or release is produced by preparation. |
| User docs/site (#98/#102) | README preparation and #507 (`ea545897`) site implementation are present. [Documentation CI](https://github.com/Androlax2/agent-cockpit/actions/runs/41388685000) passed on the snapshot. | Volunteer README-only install/session remains unmeasured; docs owner updates public-install instructions after #97. A rendered site is not onboarding evidence. |
| Diagnostics (#137) | #81 (`ea96cd83`) diagnostics library; `tests/level.rs::an_override_reverts_when_it_expires`, `tests/canary.rs::environment_values_never_reach_logs_or_bundles`, `tests/support_bundle.rs`. CLI `tests/bug_report.rs::new_asks_the_daemon_for_a_draft_and_prints_it` covers the current draft/bundle route. | No `logs level` or `support-bundle` CLI verb in current `src/main.rs`; `config reload` is configuration, not logging reload. Runtime override acceptance stays open pending an owner command or written deferral. |
| Theo acceptance (#137) | This guide and isolated fixtures are preparation only. Historical M1 runs are recorded separately in [m1-acceptance.md](m1-acceptance.md). | Theo completes the Alpha walk; findings filed, fixed and rerun or explicitly deferred on #52. Gate stays open. |

Repository protection #382 and name/assets #339/#99 are also open Alpha milestone work; they belong to their existing owners and no decision or permission change is implied by this preparation.

## Recorded offline preparation, 2026-11-02

This is a machine rehearsal, **not Theo's transcript**. Rust source was the snapshot above; only this preparation's scripts/tests/docs were changed. The development CLI reports `cockpit 0.1.0 (revision unknown: not a release build)` and no release bundle manifest, so it is not a release candidate receipt.

```text
CARGO_BUILD_JOBS=2 NEXTEST_TEST_THREADS=2 cargo nextest run --locked -p cockpit-cli --test doctor --test onboarding -j 2
Summary [  70.473s] 21 tests run: 21 passed (1 slow), 0 skipped

python3 tools/acceptance/alpha1.py --bin-dir target/debug --parent /tmp/opencode
/tmp/opencode/alpha1-rt6rnq0g/evidence.json
exit 0
```

Executable SHA-256: `a3ad40c5de20d49e7ad48563d6fe393f94dc5ac50f02fd0e03eca923231f8d3b`.
The four command exits were `0, 0, 0, 1`. Config used `next` and scratch-only paths, sandbox remained `required`, init reported `0 of 6 steps done`, and doctor reported config available / daemon missing / host unknown. The record sets `scope: offline-rehearsal` and `live_acceptance: false`. The local scratch evidence is retained, not uploaded or promised as a portable artifact.

Focused runner fixtures also cover inherited credentials/profile/socket removal, private paths, paths containing spaces, fresh homes, candidate hashing, missing executables and unexpected exits. Verification details and the independent review are recorded in the preparation PR. No native provider conformance or full workspace suite was rerun for this preparation; the baseline CI evidence above is identified separately.

Final focused fixture command: `python3 -m unittest discover -s tools/tests -p '*acceptance*.py'` — `Ran 23 tests`, `OK` (exit 0). `make docs-ci`, Ruff check and the new Python files' format check passed. Initial fixture absence (six failures), formatting differences (two files) and a reproduced long-TMPDIR test failure were corrected; no checks were disabled.

Independent review session `ses_e65626149ffeThnxo6Pcao1q8O` inspected the full six-file delta. No blocking isolation/evidence defect was found. Its test-portability finding was reproduced red and fixed by using a short fixture temporary parent; the regression now passes. This remains separate from the named review below.

### Named review receipt, 2026-11-02

`code-review medium 530` followed the installed OpenCode command definition at `~/.config/opencode/commands/code-review.md`, not a skill. The earlier unavailable-workflow report was a discovery error, resolved under cockpit blocker `650e9d2b`; no restart or installation was needed.

Scope: the complete six-file delta `ea5458970720cf24dd807c21a1cdb808085ed5a4..83193277ecf486c52252bbdc3ab72132525c445b`. All eight angles ran sequentially inline: A, B, C, Reuse, Simplification, Efficiency, Altitude and Conventions, followed by deduplication. No subagents, verifier runs or simplify were part of this named review. Its only finding was the stale unavailable-review claim in this handoff; this documentation follow-up corrects it and the implementation plan. The original review made no edits or GitHub posts; the follow-up was separately requested.

The preparation review gate is satisfied. Human acceptance, published installation and the live prerequisites above remain separate; this receipt does not close #137 or #52.

## Live run record

**No live Alpha acceptance has run.** After Theo participates, append one dated entry with:

- Candidate tag/revision, binary digest, archive digest, guide revision, OS and Docker mode.
- Consent/readiness and actual environment isolation; no credentials or personal paths in public evidence.
- Each section: passed, failed, blocked or not run, with the person's observations and command receipts.
- Finding issue URLs, explicit #52 deferrals and exact successful reruns.
- Gate comment linking this run. Never prepopulate a successful transcript.
