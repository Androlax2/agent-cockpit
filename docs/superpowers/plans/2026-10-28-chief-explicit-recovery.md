# Explicit Chief Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Recover the same dedicated Chief conversation after relaunch or reboot through fresh native verification and explicit protected host approval, never inherited chat authority.

**Architecture:** Add one host-only recovery action that atomically retires the stale grant and creates a new immutable grant for the same conversation, agent and canonical project. Reuse the existing host socket, exact-runtime admission evidence, request receipts and Chief events. No automatic restart/restore hooks and no product UI changes.

**Tech Stack:** Existing Rust, serde, rusqlite and fake-system Rust regressions; existing cockpit-team host CLI and ask-to-run deployment.

**Spec:** Option A approved by Theo in this conversation on 2026-10-28. The constraints and acceptance criteria below preserve that approval.

## Global Constraints

- Only c894851f is authorized. Other backlog, the pinned Chief card and Stadeo stay paused.
- No live Chief restart, PC reboot, native verification/grant mutation or admission weakening without separate Theo approval.
- Preserve immutable original grant identities and append auditable replacement provenance. Preserve paused state.
- Fresh verification must belong to the exact replacement runtime, not another session or an old runtime.
- No migration, dependencies, manifests, installation or automatic recovery permission is needed.
- All host commands use ask-to-run. Source changes happen in an isolated checkout based on the applied revision, not the unrelated current branch.
- Do not include a `Todo: c894851f` commit trailer while actual lifecycle evidence remains outstanding: cockpit apply would automatically close the todo.

## Review Focus

- An old launcher remains alive but has disappeared from the session registry: reject recovery using the actual process-runtime liveness check.
- A paused Chief is recovered: the replacement stays paused and cannot admit a wake.
- Approval is stale, repeated, retargeted or races another recovery: no second replacement and no reuse of authority.
- Old-runtime messages were already submitting or uncertain: do not erase that uncertainty, replay them or silently redirect them.
- A reboot restores the same chat and startup record: neither restoration nor polling grants authority before explicit recovery.

---

### Task 1: Exact host-authorized grant replacement

**Files (relative to the isolated checkout):**
- Modify: `apps/cockpit/src/chief.rs` — host action and validation.
- Modify: `apps/cockpit/src/chief/store.rs` — atomic replacement and audit events.
- Modify: `apps/osd/src/chief.rs` — authoritative liveness and native admission preflight.
- Test: `apps/cockpit/tests/chief_store.rs`, `apps/cockpit/tests/chief_events.rs`, `apps/osd/src/chief_tests.rs`.

**Interfaces:**
- Add `ChiefHostAction::Recover { request_id: String, expected_epoch: String, expected_revision: i64, session: VerifiedSession }`, serialized as `recover`.
- Keep `store::host(connection, action, live, now_ms)` and daemon `chief::host(registry, action)` signatures unchanged.
- Recovery response records `previous_epoch`, new `epoch`, replacement identity and preserved `state`. The existing request fingerprint/receipt is the idempotency boundary; an exact replay is only a receipt, never another activation.
- Add a store read function returning the exact retained grant needed by host preflight, if the existing protected snapshot/runtime reads cannot supply it without a new abstraction.

- [x] Write failing store tests: recovery produces exactly one new grant, old identity remains immutable, prior epoch is disabled, new epoch differs, approval hash derives from the recovery action, event records both epochs/identities, and paused remains paused without a queued wake.
- [x] Add failure/replay tests: stale revision/epoch, disabled old grant, different conversation/agent/project, unchanged runtime, product worker, missing exact-live replacement, repeated request with changed content, concurrent requests against one source epoch, and transaction failure leave no partial replacement. Exact replay returns only the saved result and never reactivates a later-disabled grant.
- [x] Write daemon tests: reject live old launcher even when unregistered; reject missing, wrong-session, wrong-runtime and wrong-adapter admission; reject future or expired evidence. Require admission for this exact replacement observed within five minutes of approval (implementation choice, not a standing authorization).
- [x] Run these tests red, then implement the smallest recovery branch. Check old-runtime process death at the host boundary, exact old epoch/revision and replacement identity in the immediate store transaction. Do not modify legacy Enable admission behavior as an adjacent fix.
- [x] Preserve existing delivery reconciliation semantics: cancel only provably unsubmitted queued old-epoch deliveries using existing answer cleanup where appropriate; never retarget any delivery to the new runtime. Retain submitting/uncertain attempts and their host decisions. Add regression assertions for each state.
- [x] Seed a new coordination/bootstrap record for the new grant without inheriting old turn allowances or pending request authority. Keep global decisions/habits/history readable. Audit the explicit recovery through existing Chief events and host action receipt.
- [x] Run targeted tests green plus existing Chief lifecycle, events, startup, admission and runtime-identity regressions. Reboot fixtures use a new boot/runtime, not real host lifecycle changes.

### Task 2: Reviewable host entry point, independent review and deployment

**Files:**
- Modify: `apps/cockpit/src/bin/cockpit-team.rs` — read-only recovery review mode and host-only submission confirmation.
- Test: `apps/cockpit/tests/` — use the existing CLI harness discovered in the isolated checkout.
- Update this plan with exact verification commands/results and applied revisions; no general documentation cleanup.

**Interfaces:**
- Add proposed CLI mode `cockpit-team recovery-review <session-id>`: host-only snapshot read via `chief::host_snapshot()`, select exactly one retained grant and exactly one fresh replacement worker, emit the complete proposed `ChiefHostAction::Recover` request JSON without mutation. Never choose a different conversation as fallback or automatically verify admission.
- Submit through existing `cockpit-team chief-host`, with a separate exact-payload host confirmation analogous to existing reconciliation confirmation. Both review and submission run through ask-to-run.
- Retain existing `VerifyChiefAdmission`: it must follow an actual controlled native queue/draft check on the replacement runtime, with evidence recorded by the host. The new review helper must not manufacture this evidence or attest based on chat text.

- [x] Add failing CLI/host-boundary tests: review never submits, malformed/ambiguous identities fail closed, agent and remote paths cannot recover, confirmation decline changes nothing, and revision/runtime changes between review and approval are rejected.
- [x] Implement review/confirmation wiring and make tests pass. No sidebar/phone/pinned-card code changes; no UI design work.
- [x] Independently review the diff with a fresh read-only reviewer, scoped only to c894851f and its trust boundaries. Fix findings and rerun affected tests.
- [x] Report the multi-file diff stat; run the normal clone test suite through the supported shared runner. No builds/installs beyond approved commands; request host execution where sandbox capabilities require it.
- [ ] Commit only requested source/tests/plan on the isolated feature branch. Export reviewed patches and use ask-to-run for normal cockpit apply and host tests. Deployment must not restart the live Chief or mutate its grant; stop if the deployment path would do so.
- [ ] Report tested commit, deployed commit, exact test output, and which lifecycle checks were simulated versus real. Keep c894851f open pending sufficient evidence.

## Next-boot acceptance procedure (separately approved when performed)

Restore the same conversation without authority. Through approved host commands, inspect its fresh runtime and perform a controlled native draft/queue check, then record exact-runtime admission. Review the emitted recovery request and explicitly approve it. Verify that the new grant is bound to the new runtime, the old epoch cannot act, paused status is preserved, and no old-runtime delivery was replayed. If paused, resuming coordination is a separate deliberate host action and must not undo Theo's global pause.

No real restart, reboot, verification attestation or recovery operation is authorized by approving this implementation plan.

## Implementation evidence

- Base revision: `c2f7f90fd46cedf59f8052c6ba07b241e0238e02`.
- Host-approved unchanged baseline: 53 Chief store tests passed. Sandbox offline execution could not resolve cached `base64`; no dependencies were installed.
- RED: eight store and two daemon tests failed for the missing recovery action. CLI tests exposed the absent review command and absence of separate confirmation.
- Independent read-only reviewer `ses_e8043a390ffesstMdPbB31Gj3n` found one Important issue: pending old-epoch session starts survived recovery. Both new store and daemon-path regressions reproduced it before the fix.
- The fix expires old-epoch pending request authority, retains request tokens and existing launch/reconciliation records, and audits the retired-request count. It does not undo already-started work.
- GREEN after review fix: both new regressions passed. Standard clone `CARGO_NET_OFFLINE=true make test`: `Ran 119 tests in 16.959s`, `OK`; `3119 tests run: 3119 passed (1 slow), 0 skipped` in 68.331s. Host request: `d2e9ce3dd8db`.
- No real Chief restart/reboot, replacement-runtime admission attestation or grant recovery was performed. Deployment and post-boot acceptance are still pending at this commit. Keep c894851f open.
