# Alpha 1 acceptance preparation and evidence

Status: **prepared, not accepted**. No Theo run is recorded. Issues [#137](https://github.com/Androlax2/agent-cockpit/issues/137) and [#52](https://github.com/Androlax2/agent-cockpit/issues/52) remain open.
The `m3` filenames preserve the issue links; Alpha 1 is the current release name.

## Current Alpha 1 platform scope

Theo's [2026-11-02 decision](../decisions/2026-11-02-568-alpha-platform-scope.md) limits Alpha 1 to **Linux and WSL2 through archive installation**. macOS delivery, Homebrew and macOS acceptance are **deferred, not passed**, under #568 (including #527/#564); no restoration milestone is assigned. Other packaging (#563/#565) moves to Alpha 2. The Arch AUR recipe (#562) is prepared in Alpha 1 but is not an Alpha 1 archive-install prerequisite ([decision](../decisions/2026-11-03-562-arch-packaging-alpha1.md)).

The required onboarding matrix is Linux/WSL2 × Docker present/absent: **four cells**. The two macOS cells are deferred. Linux/WSL2 sandbox enforcement and repeated launches, two-OS-user isolation, provider conformance and live round trips, context measurement, diagnostics, coexistence and Theo/volunteer observations remain required unless separately deferred in writing. Historical results below retain their original platforms and outcomes; they are not current support promises. Ordinary repository-required development checks are unaffected.

## Acceptance and publication order

The coordinator's 2026-11-03 source-order clarification applies [#97's explicit staged-before/public-after policy](https://github.com/Androlax2/agent-cockpit/issues/97) to this guide. It corrects the stale requirement to publish before the #137/#52 walk; it grants no release or live-operation approval and records no completed acceptance.

1. **Before tagging/publication:** use the exact approved staged candidate for the required installation, provider, sandbox and guide observations. All prerequisites below, the four onboarding cells, two-OS-user isolation and Theo's actual readiness-confirmed walk remain required. Record findings and successful reruns or Theo's explicit written #52 dispositions. Source delivery, CI and the offline rehearsal cannot complete #137 or justify early #52 closure.
2. **After actual #52 acceptance, before publication approval:** the release owner revalidates the final tagged artifacts, recording their archive/binary digests, manifest, source revision, version, architecture and applicable release-gate results. A staged `dev.N` build and a final `alpha.N` build have different version identities and may have different bytes; matching source does not establish identical artifacts. Re-run affected acceptance steps and findings whenever different bytes, source, behavior or check inputs invalidate the staged evidence. Keep both identities and the comparison/rerun receipts; do not silently transfer acceptance. This later release phase does not authorize a candidate dispatch, tag, release-draft merge or publication.
3. **After separately approved publication:** independently verify the exact public archive and documented install route on Linux and WSL2 for #97, and the public README/site routes and human observations for #98/#102, before closing those issues. A staged walk or final pre-publication check cannot supply that public-install proof. These later records remain distinct from the pre-publication #137/#52 walk.

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
| Candidate installation | Release lane #191/#97, using #192's completed tooling evidence: exact approved staged candidate source/version/architecture, archive and binary digests, reviewed Linux/WSL2 installation instructions and install/doctor receipts. Final tagged-artifact revalidation and independent public-install proof follow in their separate phases above. No publication or installation was performed here. |
| Sandbox/network | Greywall and Greyproxy were removed (#828): this release has no sandbox, agents run unsandboxed by default and an explicit `required` refuses every launch. Enforcement evidence returns with Torlek's own sandbox (#546 stages 2 to 5). Do not stop/reconfigure the legacy greyproxy or silently reuse it. |
| Providers | Provider lane #51/#419/#436: conformance/provenance matrix, real model sees brief/skills, exact delivered-turn approval destination. Explicit consent and clean-environment sign-in precede account usage. |
| Provider scope and remaining evidence | Theo confirmed on 2026-11-03 that Alpha 1 uses Claude, Codex and the existing OpenCode HTTP/plugin adapter; no additional ACP adapter is required, and HTTP/plugin is not ACP. Actual provider conformance and live task round trips remain required. #479 owns missing total-context token measurement. Runtime logging override lacked a CLI route at the original inventory revision; the source follow-up below supplies it, while the exact staged candidate and Theo's observation remain required. The remaining evidence is open, not silently waived. |
| Theo's participation | #137: readiness and explicit consent for the approved staged candidate, then Theo's actual observations during the walk. No Theo Alpha walk is recorded. |

The separate #98 volunteer walk uses the public install route after publication; it is required before #98 closure, not a prerequisite for starting the staged #137 walk. No such walk is recorded. Its [walkthrough record](../acceptance/98-user-readme.md) stays with the docs owner.

Issues #51 and #192 are closed with criterion evidence; consume those receipts rather than treating their issue status as an unresolved prerequisite. Their closure does not establish the separate signed-in provider observations or published archive installation. #99's logo work moves to Beta 3 design and #397's further CI efficiency work to Hardening; neither blocks this Alpha 1 walk.

Morning question, **only after those dependencies are ready or explicitly deferred on #52**:

> Are you ready for a 45–60 minute isolated Alpha 1 walk covering first run, three providers and diagnostics, plus 15–30 minutes if fresh sign-in is needed?

If dependencies remain open, Chief should present their specific missing receipt instead of opening a guide. The [#97 coordination handoff](https://github.com/Androlax2/agent-cockpit/issues/97#issuecomment-6569328690) requests candidate and platform receipts from release/docs owners. No files in those lanes are edited here.

## Evidence inventory at the inspected revision

Snapshot: `ea5458970720cf24dd807c21a1cdb808085ed5a4`, 2026-11-02. Source references below are present at that revision; a merged implementation is not live acceptance.
The remaining-gap column records what was open at that snapshot, including the then-required six-cell matrix. Apply the current scope and status updates above when scheduling Alpha 1; the macOS gaps are deferred, not successful results.
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
| Diagnostics (#137) | #81 (`ea96cd83`) diagnostics library; `tests/level.rs::an_override_reverts_when_it_expires`, `tests/canary.rs::environment_values_never_reach_logs_or_bundles`, `tests/support_bundle.rs`. CLI `tests/bug_report.rs::new_asks_the_daemon_for_a_draft_and_prints_it` covers the current draft/bundle route. Runtime wiring follow-up: `cockpit-cli` `logs::owner_can_set_inspect_and_revert_the_running_daemons_level`, `cockpit-daemon` `logs::timed_override_changes_the_installed_filter_and_expires_to_startup_baseline`, `logs::all_log_commands_are_refused_on_the_agent_socket_without_changing_level` and `logs::trace_override_keeps_fake_provider_content_out_of_logs_and_the_existing_bundle` exercise the owner command and retained content boundary. | At the original inventory revision the runtime verb was missing. The source command now exists; exact-candidate runtime override acceptance by Theo remains unmeasured. The support bundle continues through bug-report new/show; no second bundle verb is required. |
| Theo acceptance (#137) | This guide and isolated fixtures are preparation only. Historical M1 runs are recorded separately in [m1-acceptance.md](m1-acceptance.md). | Theo completes the Alpha walk; findings filed, fixed and rerun or explicitly deferred on #52. Gate stays open. |

At that snapshot, repository protection #382 and name/assets #339/#99 were also open Alpha milestone work. Their current ownership and milestone status are tracked on the issues; #99 is now deferred to Beta 3 design as noted above.

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

- Phase (staged guide, final tagged-artifact revalidation or public install), candidate version/tag if any, source revision, binary digest, archive digest, manifest, architecture, guide revision, OS and Docker mode.
- Consent/readiness and actual environment isolation; no credentials or personal paths in public evidence.
- Each section: passed, failed, blocked or not run, with the person's observations and command receipts.
- Finding issue URLs, explicit #52 deferrals and exact successful reruns.
- For a later artifact, its comparison with the staged identity, invalidated evidence and affected acceptance/finding reruns; public-install evidence stays separate.
- Gate comment linking this run. Never prepopulate a successful transcript.
