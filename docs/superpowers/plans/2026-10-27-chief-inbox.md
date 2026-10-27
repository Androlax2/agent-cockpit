# Chief inbox Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task (Theo chose inline execution in this session). Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Answer what the Chief escalates one question at a time: a short situation, the suggestion as steps done one by one, and only the last step sends the exact message.

**Architecture:** The host keeps one new fact per escalation, "later until", so desktop and phone agree and it survives restarts. Everything else is presentation over the existing host snapshot and the existing `answer_decision` action. The desktop gets an inbox frame inside the Chief view; the phone gets an inbox route in the existing remote page. Steps come from the decision record when the Chief provides them (todo `a82b3742`); until then the suggestion is one step and the Chief's reason is the situation.

**Tech Stack:** Rust (cockpit, osd), rusqlite, ratatui terminal UI, the phone page (`apps/cockpit/assets/remote/page.js`) with its Node and Chrome checks. No new dependencies.

**Spec:** `DESIGN.md` section "Chief inbox"; FigCraft `design-plans/cockpit-session-list.figcraft` page 15, frames 3097 3130 3159 3182 3204 3222 3238 (ready for dev); the Designer's answers in thread `a6e3c937` (recorded below). Todo `072be307`.

## Global constraints

- Copy, keys and colours exactly as DESIGN.md "Chief inbox" and the frames. Amber is for "needs you" and the primary key only.
- Only the last step sends, and only the message shown. Enter never sends. A step never runs a command.
- Desktop send is `y` on the last step, `⌘S` in Write my own. Phone sends need a second tap.
- `l` later hides a question 20 minutes on desktop and phone alike, never answers it, and survives restarts.
- Never moves on by itself after a send: `n` next question, `esc` stop here.
- `i` / "Full details" keeps the raw source, category, reason and the untrusted-narrative note (the existing detail).
- Chief text is untrusted narrative: escape it, label it "Written by the Chief: check it before acting."
- Scope (Theo, Option A): entry is Status → Chief (desktop) and the phone's Chief area. The desktop session-card ask stays deferred.
- Helper keys (`g` show the guide, `o` open the session) and the Chief-provided situation/steps belong to `a82b3742`. This plan renders steps from the record when present, with no helper action.
- Preserve existing Chief views, answers, habits, startup and recovery flows.

## Designer answers (thread a6e3c937)

1. Phone runs the same stages as desktop, one screen each: step card + "Done, next step" (amber) + "Back to the question"; last step = message + "Send" (amber, second tap) + "Edit first"; write my own = text box + "Send" (second tap) + "Back"; sent = "✓ Sent to …" + next preview + "Next question" / "Stop here".
2. Step action kinds are an allow-list owned by the Cockpit; unknown or missing = plain step. (Out of scope here, see above.)
3. `e` edit first opens Write my own prefilled with the message; Esc returns to the last step with it unchanged.
4. Later is host-side.
5. "Then" lists only remaining steps, dim; done steps show as ticked dim lines above. Ticks live per question in memory until answered or gone.

## Review focus

1. The question changes or is answered elsewhere while Theo is on a step: the send must refuse (stale revision) and the inbox must show the current state, never send to a resolved question (Task 1 host check, Task 2 refresh test).
2. A long situation, step or message, and a narrow column: nothing is cut silently; text wraps and the keys stay visible (Task 2).
3. Later from the phone, then the desktop opens the inbox: the question is hidden on both, and returns after 20 minutes (Tasks 1 and 3).
4. An escalation without a suggested answer: no `a`, Write my own still works (Tasks 2 and 3).
5. Double send on the phone (two quick taps, flaky network): one answer at most (Task 3, existing pending-post guard plus second-tap arming).

---

### Task 1: Host "later" for Chief questions

**Files:** `apps/cockpit/src/chief.rs`, `apps/cockpit/src/chief/store.rs`, `apps/cockpit/src/teams/store.rs`, `apps/cockpit/src/teams/links.rs` (version guard), `apps/cockpit/src/remote.rs`, tests `apps/cockpit/tests/chief_protocol.rs`, `apps/cockpit/tests/chief_store.rs`, `apps/cockpit/tests/claude_remote.rs`.

**Interfaces:**
- Produces: `ChiefHostAction::LaterDecision { request_id: String, decision: String, expected_revision: i64 }`; decision records gain `later_until_ms` (null when not deferred) in the host snapshot and in the phone projection `chief_display_record`.
- Constant: `CHIEF_LATER_MS: i64 = 20 * 60 * 1000`.

- [ ] Write failing tests: protocol parses `later_decision`; store test: later on an open escalation sets `later_until_ms = now + 1_200_000`, keeps it open and its revision unchanged; a resolved or stale-revision decision is refused with Conflict; answering still works while deferred; migration v16→v17 keeps existing decisions (populated fixture); phone POST `/api/chief` with `later_decision` against the exact displayed record succeeds, and with a changed record is refused.
- [ ] Run `cargo nextest run --release --manifest-path apps/Cargo.toml --test chief_protocol --test chief_store --test claude_remote`: new tests fail.
- [ ] Implement: `upgrade_to_17` adds `later_until INTEGER` to `chief_decisions`; bump both version guards; the store action updates only `later_until` for an open escalation at the expected revision; project `later_until_ms` in `review_records` and `chief_display_record`; allow `LaterDecision` in `remote::chief_action` keyed on the decision record like AnswerDecision.
- [ ] Run the same command: all pass. Commit.

### Task 2: Desktop inbox

**Files:** `apps/cockpit/src/chief/inbox.rs` (new: stages, steps and rendering lines; pure), `apps/cockpit/src/chief/view.rs` (frame wiring, entry), `apps/cockpit/src/app/chief_ui.rs` (effects, drawing), `apps/cockpit/tests/chief_ui.rs`.

**Interfaces:**
- Consumes: Task 1 `LaterDecision`, `later_until_ms`.
- `inbox::steps(decision: &Value) -> Vec<String>`: the record's `steps[].text` when present (last = message), else `[suggested_answer]`, else empty.
- `inbox::situation(decision: &Value) -> String`: `situation` when present, else `recommendation`.
- Stages: `Question`, `Step(k)`, `Last`, `Write { text, from_last }`, `Sent { to, next }`, `Empty`.

- [ ] Write failing tests through the real App (existing `Host` fixture):
  - Status → Chief with one open escalation opens the question card: "! Chief needs you · 1 of 1", `project › task` from `session_names`, category in words and age, "What's going on", "◆ Chief suggests · N steps", the four keys and the untrusted note.
  - With no open (or only deferred) escalation, Status → Chief opens the existing root (startup screen when off); `c` on the root's "Needs you" with none shows the Empty card.
  - Two-step record: `a` → "Step 1 of 2" card, `d` → last step with "✓" first step and the message; Enter sends nothing; `y` sends exactly one `answer_decision` with the shown message and the record's revision; then the Sent card with "n next question" / "esc stop here" and no automatic move.
  - Fallback record (no steps): one step = suggested answer, situation = reason; no suggestion → no `a`, `w` still works.
  - `e` opens Write my own prefilled; Esc returns to the last step unchanged; `⌘S` sends the edited text.
  - `l` sends one `later_decision` and moves to the next question (or Empty); the deferred one is not counted in "1 of N".
  - The record changes revision while on a step: the next refresh returns to the question with a notice and `y` cannot send the stale revision.
  - 40×18 column: long situation/message wrap, the current keys stay visible.
- [ ] Run `cargo nextest run --release --manifest-path apps/Cargo.toml --test chief_ui`: new tests fail.
- [ ] Implement inbox state and rendering; reuse the startup-screen drawing approach (clear area, wrapped lines, key caps, blue outline for the current step), existing colour constants mapped to the frame tokens, `answer_action` for sends, `Effect::Submit` path without the old review frame (the last step is the review).
- [ ] Run the same command: all pass, existing Chief UI tests included. Commit.

### Task 3: Phone inbox

**Files:** `apps/cockpit/assets/remote/page.js`, `apps/cockpit/assets/remote/style.css` (if styles are there), tests `apps/cockpit/tests/remote_mobile_ui.mjs`, `apps/cockpit/tests/remote_chat_browser.mjs`.

**Interfaces:** Consumes Task 1 projection and action; route `#chief=inbox[&decision=<id>]`.

- [ ] Write failing Node checks: the Chief status card shows "Needs you · N" opening the inbox; the session view's escalation card has "Do this, step by step" opening the inbox on that decision; question card content (frame 3238); step → "Done, next step"; last step "Send" arms on first tap and posts on the second, exactly once even with a double tap; "Edit first" prefilled; "Later" posts `later_decision`; Sent card waits for "Next question".
- [ ] Run `node apps/cockpit/tests/remote_mobile_ui.mjs`: new checks fail.
- [ ] Implement with existing `element`, `post`, pending-post and `chiefSubmitted` helpers; 44px buttons per the frame; reuse the existing phone escalation card's data only.
- [ ] Run the Node suite and `REMOTE_CHAT_BROWSER=/usr/bin/google-chrome-stable node apps/cockpit/tests/remote_chat_browser.mjs` (390 and 320 px): pass. Commit.

### Task 4: Verify, review, hand off, apply

- [ ] `make test` in the worktree; record exact results.
- [ ] Fresh reviewer on the whole branch with this plan's Review focus; one fix pass with RED→GREEN tests.
- [ ] Mark frames 3097–3222 completed after desktop verification and 3238 after phone verification; keep `072be307` open only if something is left.
- [ ] Apply through `cockpit apply` with Theo's approval; run `make -C ~/agent-cockpit test` after.
