# Chief observed-work wake repair

> **For agentic workers:** Use the executing-plans workflow inline, with TDD and final independent review.

**Goal:** An idle, exactly admitted active Chief can receive its automatic coordination wake while its own coordination todo remains in progress.

**Evidence:** Host diagnostic `29fe521ae238` found wake `aaa80f4cc5e0d6671f00fef293f41ad6` queued with zero attempts and exact grant/runtime/epoch match. Chief readiness was blocked only by `legacy_work_in_progress`. Diagnostic `5bc3350e1c03` identified an `observed` reservation for the Chief's own `cockpit-chief` task `fdafd33f`, with no associated legacy launch. The current `wake_ready` rejects every blocked worker.

## Scope and constraints

- Edit only the confirmed wake-readiness defect and its real-entry-point regressions, after related owners confirm no overlapping implementation.
- Proposed exception: only the exact active/admitted Chief, blocked solely by an observed reservation in its own project. Preserve that reservation and todo ownership.
- Do not ignore actual legacy/dispatch reservations, pending or uncertain launches, reservations in another project, product/team-worker membership, native activity/input, stale observations, viewed-session/gaming/resource gates, prompts, restarts, or unverified/stale grant identities.
- Preserve every queue/receipt and the current native-turn budgets. No manual wake claim, replay, raw database/socket mutation, reset/recovery/restart or new delivery check.
- No sites-crema inspection, dependencies or manifest changes. Host operations and deployment require normal approval. Heavy verification must pass current resource admission.

### Task 1: Narrow wake readiness and prove the automatic route

1. Confirm ownership with e2c4f5f2 and c1ecab09 owners. The task branch was synchronized to applied `1c56fd7` through approved request `73d1bafcca00`.
2. Add a failing daemon Receive-route regression with an active exact-runtime grant, verified admission, fresh native/focus facts and a real observed Chief-owned todo reservation. Assert the queued wake is automatically claimed, without deleting the reservation.
3. Add negative cases for real launches/work reservations and native/viewed/gaming/identity/admission holds. Keep an ungranted enable candidate from using the exception.
4. Change `apps/cockpit/src/chief/store.rs::wake_ready` narrowly; do not alter global legacy busy classification or reservation reconciliation. Reuse existing checks and keep effects greppable.
5. Run focused tests through `agent-build`, then affected checks and the shared full test entry point under resource admission. Complete the required review/simplify gates; report missing prerequisites or failures rather than claiming success.
6. Commit/export only this task through existing repository permissions and request reviewed host apply/verification. Do not ship other owners' changes.
7. Live acceptance: automatic same-Chief wake delivered; bounded Context consumed; a routine answer received by its original worker; resumed work supported by artifact/test evidence; then a subsequent automatic wake, without Theo prompting or guard bypass. Clearly separate fixture results from live proof.

## Approval boundary

Theo approved Option A in chat. It changes no grant or role authority, but must not become a broad bypass of work ownership. If another owner already implements the same path, coordinate instead of duplicating it.
