# Automatic Team Recruitment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatically recruit missing teammates in every project, without broadening the originating task or granting additional permissions.

**Architecture:** Activate validated recruitment in the existing store transaction, reusing the mapping and spawn checks. The daemon starts queued recruits immediately through its existing host-side launcher. Existing-session linking remains manually approved: automatic recruitment must not commandeer another running session.

**Tech Stack:** Rust, rusqlite, existing daemon and cockpit integration tests.

**Spec:** Approved chat scope for todo `ebe0496d`: global automatic recruitment; existing task, team-size, rate and permission rules remain; visible without blocking. This supersedes the manual recruitment approval requirement in `docs/superpowers/specs/2026-10-21-agent-requested-teams-design.md`, not its other constraints.

**Execution status:** Both tasks implemented. Independent review identified three edge cases; each was reproduced with a failing test and fixed. After isolating this feature onto current main and preserving its sleep and QA safeguards, final `make test` passes: 77 Python and 1,919 Rust tests, zero failures. Host apply remains. Cargo commands below run with `--manifest-path apps/Cargo.toml` from the repository root.

**Review adjustments:** Only an actual existing role holder may bypass role reservations. The daemon supplies only existing launch directories to the store. The current default agent is optional at the store boundary: persisted replays do not need readable current settings, but new requests still fail without them. Identical replays preserve the original stored agent even after settings change.

**Instruction handoff blocker:** The cross-project dotfiles instruction update was acknowledged by `cockpit-todo add`, but absent from `cockpit-todo list --all`. A blocked report was submitted; instruction alignment is not confirmed queued. The code does not change global agent instruction files.

## Global Constraints

- Automatic recruitment everywhere; no per-project setting.
- One Designer, one Developer and one QA per team, retaining role reservations for launches in progress.
- Keep authenticated sender identity, project/task membership checks, rate limits and host launch proofs.
- Briefs are collaborator instructions for the original task, never approvals for tools, installs, Git writes, scope changes or host commands.
- QA still starts per handoff, in an isolated copy at the exact commit; preserve fresh-review and session-cleanup behavior.
- No dependency, launcher, Makefile or sandbox-permission changes.
- Do not silently approve old pending requests or change existing-session linking.

## Review Focus

- Repeating a successful recruitment after membership changed must return its original result, without duplicating launches.
- A second request cannot take a role reserved by an in-flight spawn or overwrite an existing standing QA assignment.
- Invalid/missing working directories must leave no partial team, proposal or spawn records.
- Daemon restart must recover committed pending spawns without requiring the Team view to open or starting an already-started spawn twice.
- Agent-written instructions must not claim personal approval or authorize unrelated tasks.

---

### Task 1: Activate recruitment atomically and launch it immediately

**Files:**

- Modify: `apps/cockpit/src/teams/store.rs`
- Modify: `apps/osd/src/teams.rs`
- Test: `apps/cockpit/tests/team_store.rs`
- Test: `apps/osd/src/tests.rs`

**Interfaces:**

- Change `store::recruit` to accept verified directories before `now_ms`: `recruit(connection: &mut Connection, sender: &VerifiedSession, recruitment: &super::Recruitment, agent: Option<&str>, live: &[VerifiedSession], directories: &HashMap<String, String>, now_ms: i64) -> Result<TeamReply, TeamError>`.
- Preserve recruitment response fields `proposal_id`, `revision`, `mapping_hash`; add `status: "approved"` to indicate activation. Keep stored proposal status `approved`, avoiding a schema migration.
- Reuse private `approve_mapping` inside the recruitment transaction; no sandbox-accessible host action.

- [ ] **Step 1: Write failing store regressions.** Replace the pending-recruit expectation with assertions that the asker is a member, the proposal is approved, Designer/Developer spawns are queued and QA remains a standing brief until handoff. Add separate cases for exact replay (same response and one spawn), altered replay (Conflict), occupied/in-flight roles (Conflict), existing standing QA (Conflict), different task/role/project (refused), missing directory (no new proposal/member/spawn), invalid briefs and the existing ten-requests-per-minute boundary. Parameterize rejection cases where practical. Keep manual link approval tests unchanged.
- [ ] **Step 2: Run** `cargo test -p cockpit --test team_store`; confirm the new automatic-recruitment assertions fail before implementation.
- [ ] **Step 3: Implement atomic activation.** Move replay lookup ahead of membership-dependent checks so a successful retry remains idempotent. Validate new requests normally, insert the proposal and call `approve_mapping` before committing. Preserve the response hash/revision fields. Prevent standing QA replacement by a second recruitment. Update recruitment test helpers to supply verified directories and remove now-unnecessary explicit approvals; retain legacy pending-recruit rejection/approval tests using explicitly seeded historical pending records, rather than weakening their assertions.
- [ ] **Step 4: Add daemon regressions.** Recruitment alone must start exactly one detached teammate in the verified sender's directory using the configured default agent, without `host_control`. Replay must not relaunch. QA-only recruitment must not launch until handoff. Existing link proposals must still be pending. Exercise failed starts, launch-token binding, pending-spawn recovery and unreadable default settings through existing real daemon entry points.
- [ ] **Step 5: Wire the daemon.** Compute verified directories before borrowing the database, pass them to `store::recruit`, and trigger `start_spawns` after successful recruitment as well as handoff. Adapt `recruit_and_approve` test helpers to recruit without approval and rename them accordingly. Preserve manual host control for legacy proposals and links.
- [ ] **Step 6: Run** `cargo test -p cockpit --test team_store` and `cargo test -p osd`; require zero failures, including authentication and recovery coverage.

### Task 2: Preserve authority boundaries and document the new policy

**Files:**

- Modify: `apps/cockpit/src/teams/store.rs`
- Modify: `apps/cockpit/src/teams.rs`
- Modify: `apps/cockpit/src/bin/cockpit-team.rs`
- Modify: `README.md`
- Test: `apps/cockpit/tests/team_store.rs`
- Test: `apps/cockpit/tests/team_client.rs`
- Test: `apps/cockpit/tests/team_ui.rs`
- Test: `apps/osd/src/tests.rs`

**Interfaces:**

- Keep `qa_brief_text(standing: &str, handoff: &Value, findings: &[String]) -> String` and existing protocol JSON unchanged.
- Reuse existing Team view membership and failure rows; no UI redesign or new controls.

- [ ] **Step 1: Add failing authority regressions.** Both Designer/Developer launch briefs and the QA standing-instruction block must identify agent-written collaborator content, restrict work to this team's original task and explicitly deny grants of tool use, scope changes, installs, permissions, Git writes, merges or releases. Assert neither claims `approved by Theo`. Update exact old brief expectations to the new policy, preserving their behavioral checks.
- [ ] **Step 2: Run** `cargo test -p cockpit --test team_store` and `cargo test -p osd`; verify failures are the intended brief-content changes.
- [ ] **Step 3: Update briefs and help.** Remove personal-approval claims from spawn and QA briefs, keep each requested brief intact beneath the authority boundary, and describe automatic recruitment in `cockpit-team --help`. Retain the manual approval language specifically for linking existing sessions.
- [ ] **Step 4: Verify nonblocking visibility.** Add a regression that an activated recruitment snapshot shows its asker/members and recorded spawn state while adding no pending-proposal attention; failed starts must still attract attention. Use existing Team view rows without changing layout.
- [ ] **Step 5: Update the README recruitment paragraph** to state global automatic recruitment, default-agent choice, original-task restriction and unchanged approvals. Global agent instructions live in dotfiles: file an explicitly requested cross-project follow-up for Claude and OpenCode to remove the obsolete recruitment-approval instruction, without changing their unrelated consent rules. Do not edit the running dotfiles copy from this checkout.
- [ ] **Step 6: Run** `cargo test -p cockpit --test team_store --test team_client --test team_ui` and `cargo test -p osd`, then `make test` only with explicit authorization for any build commands it invokes. Record actual results and show `git diff --stat`. Commit requested changes on this feature branch with `Todo: ebe0496d` only after verification, export patches, and retain the required host `cockpit apply` approval and host test run.

## Execution recommendation

Native execution was selected: the two tasks share recruitment tests and brief expectations. Review the completed change before applying it.
