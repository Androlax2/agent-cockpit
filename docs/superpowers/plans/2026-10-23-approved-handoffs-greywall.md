# Approved handoffs and pinned greywall deployment

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver explicitly requested cross-project work despite autonomous filing caps, report actual persistence, then deploy and canary greywall fbf41fa71e5e5188a8c25d75368ac2d7ec3bcd4f.

**Architecture:** Keep autonomous caps. The daemon retains an immutable copy of each identified filing request outside sandbox reach and routes its eventual receipt to the sender's read-only answers folder. A host-only approval command requeues that exact request with a trusted cap exception; the report socket strips any claimed exception. The cockpit remains the sole list writer, saving before issuing an accepted receipt.

**Tech Stack:** Existing Rust CLI, OSD daemon, JSON state and queue, Bash deployment scripts, Go greywall tests.

**Spec:** User's deployment request and approved Option A in this conversation.

## Global Constraints

- No dependency-manifest changes, autonomous limit removal, or agent-controlled approval flags.
- Preserve reviewed permissions: no shared write grants to ~/.skills, ~/.agents, ~/AGENTS.md, ~/.config/gh or ~/.local/share/gh.
- Preserve launcher protections for modified/untracked env files.
- All host actions through ask-to-run; install supporting binary before applying preserveEnvGlobException: true to the saved Claude profile.
- Canary before restarting affected sessions; no reboot, NAS connection, key/agent exposure, UID-map widening or host permission changes.
- Work only in the clone; review and apply commits through cockpit apply. Do not push.

## Review Focus

- Forged approval or answers destinations from a sandbox must never grant a cap exception or expose another project's output.
- Duplicate retries and repeated approvals return the existing task ID, not another deployment item.
- Failed persistence never yields an accepted receipt or leaves an unsaved item eligible for dispatch.
- A missing/old daemon or delayed cockpit reports pending/unknown, not saved or queued.
- Regeneration must retain the env exception without restoring removed shared grants.

---

### Task 1: Saved filing receipts and exact-task approval

**Files:**
- Modify: `apps/cockpit/src/todo.rs`, `apps/cockpit/src/approval.rs`, `apps/cockpit/src/bin/claude-todo.rs`
- Modify: `apps/osd/src/registry.rs`
- Test: `apps/cockpit/src/tests.rs`, `apps/cockpit/tests/claude_todo.rs`, `apps/osd/src/tests.rs`

**Interfaces:**
- Retain existing todo-update transport and existing add/propose syntax.
- Add `cockpit-todo approve-request <12-hex-request-id>` for host execution through ask-to-run, and `cockpit-todo receipt <12-hex-request-id>` for checking delayed outcomes.
- Requests carry a validated request ID; the daemon owns started_in, receipt destination and trusted approval metadata.
- Receipts contain request ID, outcome, task ID when saved, destination, queued/waiting/proposed state or rejection reason. Output distinguishes transport acknowledgment from persistence.
- Approval reads only the daemon's protected request copy and appends that exact add request to the existing protected queue. It does not edit todo.json or accept replacement task text.

- [ ] Write and run failing entry-point tests: capped autonomous request is refused; host-approved exact request saves and queues despite the cap; forged socket approval remains capped.
- [ ] Write failing tests for request ownership, bad IDs, duplicate retry/reapproval, rejected destination, save failure rollback, and truthful pending CLI output.
- [ ] Implement immutable request storage and sender-scoped receipts using the existing answers layout. Keep request/queue size bounds and cap automatic requests as before.
- [ ] Implement host approval and shared deduplication; accepted or duplicate receipts include the persisted item ID. Use fallible saving and restore prior list state on failure.
- [ ] Implement receipt reporting in the CLI. Plain daemon `ok` must not print saved/queued; a timeout prints request ID and how to check or approve it.
- [ ] Run targeted cockpit and daemon tests, then the full repository tests. Record output and failures without suppressing them.
- [ ] Review the diff, commit only requested changes on the existing feature branch, export patches, and request cockpit apply through ask-to-run. Run host tests after application.

### Task 2: File and verify the single deployment task

**Files:** Existing host todo state only, through the reviewed CLI and cockpit writer.

- [ ] Refresh the target project's list and inspect any open greywall deployment items before filing.
- [ ] Reuse an existing matching open item; otherwise file one deployment task with the pinned commit and the user's constraints in its notes.
- [ ] Exercise the approved cross-project route with the real sender only if that sender is available; do not impersonate greywall from this project. Obtain per-task host approval when capped.
- [ ] Check the saved list and queued state independently of the submission acknowledgment. Record the saved task ID and claim it before deployment so another worker does not deploy concurrently.

### Task 3: Review the pinned binary and saved profile

**Files:**
- Modify: `packages/greywall/build.sh` (pinned revision and verified source archive hash)
- Modify: `config/greywall/.config/greywall/learned/claude.json` (exception only; reviewed grants unchanged)
- Test: `apps/cockpit/tests/greywall_launcher.rs`

- [ ] Refresh the clone against origin using the authorized normal clone workflow before implementation, preserving any user work. Inspect current pins and reviewed profile, which may be newer than this session's initial checkout.
- [ ] Obtain and review the exact Androlax2/greywall dotfiles commit and its preserveEnvGlobException implementation/tests. Verify archive checksum rather than inventing it.
- [ ] Add regression assertions that the Claude profile opts in without adding shared writes; retain the existing no_agent_profile_brings_back_the_env_glob_the_launcher_narrows test and env-file launcher tests.
- [ ] Update only the greywall pin/hash first. Commit/export/review/apply through ask-to-run; the existing greywall-patch workflow tests, builds and installs the supporting binary, retaining its previous binary.
- [ ] Verify installed --version identifies the full pinned commit before applying the saved-profile exception in a second reviewed commit. Do not regenerate the host profile blindly.
- [ ] Apply preserveEnvGlobException: true in the field supported by the reviewed fork; preserve every reviewed filesystem permission and account for drift acknowledgment using the fork's actual implementation.
- [ ] Point out changes to the host-executed build script explicitly during review.

### Task 4: Canary, drift checks, then controlled restarts

**Files:** Existing launcher tests and fork regression tests; temporary isolated fixtures only.

- [ ] Run host repository tests and the exact no_agent_profile_brings_back_the_env_glob_the_launcher_narrows regression.
- [ ] Run the fork's drift/regeneration tests against the pinned source with the reviewed profile permissions as assertions; compare output grants, not just the exception flag.
- [ ] Through ask-to-run, start a disposable ordinary-project canary and a linked-worktree canary using the real launcher path. Check sandbox startup and launcher env protections without reading keys or exposing agents.
- [ ] Run ssh -G against a harmless offline fixture/config target inside the canary; make no SSH connection and print no secrets.
- [ ] Only after canaries pass, identify affected sessions and request their controlled restart through ask-to-run. Verify resulting session state/startup rather than counting a restart acknowledgment.
- [ ] Refresh todo list, close the verified deployment task, and report saved ID, installed commit, exact test results, canary/restart results and anything still blocked.
