# Cockpit Task Continuity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make unseen work, queue decisions, task history, return notes and deferred attention usable in the existing terminal cockpit.

**Architecture:** Add reliable completion/repository metadata to the existing attributed hook→daemon→snapshot protocol. Keep continuity state and pure decisions in focused Rust modules, with explicit App call sites and named actions in the existing palette. Queue explanations and dispatch share their evaluation, while result/task history remains separate from activity state and todo retention.

**Tech Stack:** Rust 2024, Ratatui, crossterm, serde/serde_json, existing hashing dependencies, Bash/jq, tmux, existing GitHub fetch thread and test fixtures. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-20-cockpit-continuity-design.md` (approved by Theo).

## Global Constraints

- Keep Rust, Ratatui, tmux, the four existing panels, stable session ordering, current colours and focus behaviour. No new permanent panel.
- No dependency additions or manifest changes.
- No automatic commits, pushes, merges, applies, approvals, or answers **in product behaviour**. Development commits and host-reviewed dotfiles apply follow the repository authorisation.
- Preserve terminal-session exclusions, external-session restrictions, designer routing, the viewed-session queue exclusion, and gaming pauses.
- No web/phone feature parity, new desktop notification subsystem, global agent instruction changes, inferred task dependencies, or AI-generated summaries.
- New user state is cockpit metadata, never instructions delivered to agents.
- Twenty minutes is the only deferral duration; return notes accept at most 280 Unicode characters.
- Existing single-key shortcuts retain their meanings. New operations use named `:` actions.
- The hook runs inside the sandbox; desktop reporting and applied binaries participate in host execution. Call those changes out explicitly before host-reviewed apply.
- No manifest, Makefile, launcher, compose, or global hook-instruction changes. The approved session-state hook/report additions and existing todo handoff note are in scope.
- Do not install dependencies or run a build without explicit approval. Plan approval must include test/build runs before execution starts.

## Review Focus

1. A completion followed immediately by another turn must remain unseen even while current activity is Working; identical outcomes and duplicate Stop reports must not collapse distinct turns or duplicate one turn (Tasks 1, 3).
2. A stale result from task A must never become task B's result when one session is reused; old or closed tasks must remain distinguishable from live records (Task 5).
3. A waiting session with a question, permission request, queued manual message, or command still running must not be mistaken for freely available queue capacity (Task 4; preserve manual-message ordering).
4. A question temporarily superseded by a command approval must not reappear with its old deferral; new requests must remain visible without allowing any command to run (Task 6).
5. Invalid/oversized/symlinked state or failed persistence must preserve the prior file and surface a lasting error, without claiming a note was saved or an update was acknowledged (Tasks 2, 3, 6).

## File map and test commands

| File | Responsibility |
| --- | --- |
| `config/claude/.claude/hooks/session-state.sh` | Stable turn id/completion marker and sandbox-derived repository/branch reports |
| `apps/desktop/src/registry.rs`, `apps/desktop/src/tests.rs` | Validate, attribute, retain, restore and publish the added metadata |
| `apps/cockpit/src/sessions.rs` | Shared result/repository metadata on Session and snapshot decoding |
| `apps/cockpit/src/continuity.rs` (new) | Versioned sidecar state, observations, acknowledgement, notes, assignments and deferrals |
| `apps/cockpit/src/queue.rs` (new) | Pure shared eligibility and candidate policy; no side effects |
| `apps/cockpit/src/app/continuity_ui.rs` (new) | Explicit App continuity actions, detail/picker content and task navigation |
| `apps/cockpit/src/app.rs` | Tick/action/render wiring, summary, cards and shared queue dispatch |
| `apps/cockpit/src/paths.rs`, `apps/cockpit/src/lib.rs` | Sidecar path and explicit module exports |
| `apps/cockpit/src/todo.rs` | Existing handoff note, task/session metadata access |
| `apps/cockpit/src/reviews.rs` | Preserve exact head branch/revision and cached fetch availability |
| `apps/cockpit/src/tests.rs` | Real App/Screen/render/persistence integration tests |
| `apps/cockpit/tests/session_state_hook.rs` | Actual Bash hook entrypoint and emitted socket reports |
| `apps/cockpit/tests/claude_todo.rs`, `apps/cockpit/tests/opencode_plugin.rs` | Existing command/plugin expectations where additions affect them |
| `README.md` | Discoverable named actions and honest scope/retention descriptions |

Use the existing CPU guard for every Cargo invocation:

```sh
python3 config/bin/.local/bin/agent-build cargo test --manifest-path apps/Cargo.toml -p cockpit --lib continuity
python3 config/bin/.local/bin/agent-build cargo test --manifest-path apps/Cargo.toml -p cockpit --test session_state_hook
python3 config/bin/.local/bin/agent-build cargo test --manifest-path apps/Cargo.toml -p desktop --lib continuity
```

Prefix new tests with `continuity_` (or put model tests in `continuity::tests`) so the filter selects them. Run each new test in its owning binary before implementation, record its actual failure, then rerun it after implementation. Do not mistake `0 tests` for a pass. All tests use scratch state and fake external commands, never real session state.

## Task 1: Reliable completion and repository reports

**Files:** Modify the hook, `registry.rs`, `sessions.rs`, `apps/desktop/src/tests.rs`, and `tests/session_state_hook.rs`; update affected OpenCode protocol expectations if needed.

**Interfaces:**
- Add `TurnResult { id: String, completed_at: i64, outcome: String, repository: String, branch: String }` in `sessions.rs` with serde support and bounded validation.
- Add `Session.repository: String`, `Session.branch: String`, `Session.last_result: Option<TurnResult>`; corresponding daemon Entry fields and optional Report `repository`, `branch`, `turn_id`, `turn_complete`.
- Reports carry `turn_id` as 32 lowercase hexadecimal characters and `turn_complete: true` only on Stop. `last_result.completed_at` is host-observed Unix seconds, independent of activity `since`.
- Add `sessions::github_repository(remote: &str) -> Option<String>` for recognised HTTPS/SSH GitHub forms, normalised to lowercase owner/repo. Expose the same pure normalisation rules to hook tests; do not run host Git in agent checkouts.

- [ ] **Step 1: Add failing protocol tests.**

  | Test | Assertions |
  | --- | --- |
  | `continuity_stop_reuses_its_turn_id_but_a_new_prompt_gets_another` | Prompt→Stop→Stop reports the same nonempty 32-hex id; next Prompt→Stop has a different id even with identical outcome text |
  | `continuity_a_stop_question_still_reports_completion` | NeedsInput→Stop reports both `needs_input=true` and `turn_complete=true` with outcome |
  | `continuity_fallback_completion_does_not_reuse_a_prior_turn` | Stop without Prompt gets an id; duplicate Stop retains it; subsequent real work gets another |
  | `continuity_repository_and_branch_are_read_inside_the_sandbox` | Hook shim sees repo discovery only inside HookTest; GitHub SSH/HTTPS normalise identically; local/invalid origins yield no GitHub identity |
  | `continuity_results_survive_new_work_and_daemon_reload` | Identified completion persists as `last_result` across working state and snapshot reload; legacy `outcome` behaviour remains unchanged |
  | `continuity_duplicate_completion_preserves_the_first_result_timestamp` | Repeated id produces no new result/time; subsequent distinct id does |
  | `continuity_metadata_cannot_change_authenticated_sender_routing` | Spoofed cwd/repository/target cannot change sender attribution or access another project's todo/approval channel |
  | `continuity_old_and_malformed_reports_fail_conservatively` | Old producer remains accepted without fake completion; wrong types, bad turn id and control/tmux sequences rejected; completion requires id and outcome fields |

- [ ] **Step 2: Run the new hook and daemon tests; record the expected failures.**
- [ ] **Step 3: Implement the protocol.** Persist per-session turn token/completed status alongside existing hook cache files; reset on a genuine turn start, keep duplicate Stop idempotent, and use existing cleanup on SessionEnd. Read origin/branch on SessionStart and Stop inside the sandbox. Retain and restore `last_result` separately from `Entry.outcome`, including completion-with-question; no banner policy change. Validate added fields in both report parsing and snapshot loading. Preserve optional-field compatibility for both agent payloads.
- [ ] **Step 4: Run both owning suites and OpenCode hook/plugin coverage.** Expected: all new tests and existing tests pass, with no existing outcome/assertion disabled.
- [ ] **Step 5: Commit only Task 1 files.** Message: `feat: report stable cockpit turn results and repository identity`.

## Task 2: Persistent return notes and safe state

**Files:** Create `continuity.rs` and `app/continuity_ui.rs`; modify `lib.rs`, `paths.rs`, `app.rs`, `tests.rs` and README.

**Interfaces:**
- Add `paths::continuity() -> PathBuf`, returning `state_dir()/continuity.json`.
- Add `Continuity::load(path: &Path) -> Result<Continuity, String>` and `Continuity::note(&self, session: &str) -> Option<&str>`.
- Add `Continuity::set_note(&mut self, session: &Session, note: Option<&str>) -> Result<bool, String>`: validate and persist before replacing memory; return whether state changed.
- Add `App.continuity: Result<Continuity, String>` and `App::run_continuity_action(&mut self, name: &str, screen: &mut dyn Screen) -> String` in the child module. Existing `run_palette_only` explicitly routes new action names to it.
- Sidecar JSON uses schema version `1`, typed records with flattened unknown JSON fields, maximum file size 16 MiB and at most 10,000 session/task/PR records each. At limits, fail visibly rather than prune user history. Use `read_untrusted` with limit+1 so oversize is distinguishable from malformed JSON.

- [ ] **Step 1: Add failing state and real palette tests.**

  Example action wiring (new helpers must exercise `handle`, not bypass it):

  ```rust
  app.handle(Key::Char(':'), &mut FakeScreen::typing(&[
      "1\treturn-note", "Review the VAT migration before approving.",
  ]));
  assert!(texts(&app, 1).join("\n").contains("Your note:"));
  let reloaded = fixture.app();
  assert!(texts(&reloaded, 1).join("\n").contains("Review the VAT migration"));
  ```

  Add separate `continuity_` tests for edit/cancel/explicit clear; accepting 280 Unicode characters and rejecting 281; controls and tmux escape rejection; stable notes across view switches and session exit; preserving unknown fields; refusing malformed/unsupported/oversized/symlink/FIFO state without rewriting it; `0600` atomic replacement; failed rename/write leaving the prior note and file intact. Cancel is not the clear operation.

- [ ] **Step 2: Run the filtered cockpit library tests; record the expected failures.**
- [ ] **Step 3: Implement only the state foundation and return-note actions.** Missing file creates an empty in-memory version-1 state. Other load errors remain visible across ticks and block mutations, preserving the original bytes. Owner-only, random `create_new` temporary files and rename provide atomic save; remove only owned temporary files on failure. Do not alter global `util::write_json` or unrelated persistence. Write only after a real change. Add `return-note`/`clear-return-note` palette actions; use existing input facilities and show the note in selected/full details. Audit note mutations with text fingerprints only. Retain exited-session notes in the sidecar for later archived details.
- [ ] **Step 4: Run the filtered tests and existing cockpit library suite.** Expected: notes persist, no save success on failure, existing keys/layout tests remain green. Add narrow/short render cases at `(30,12)`, `(60,24)` and `(100,40)` asserting no panic and palette access.
- [ ] **Step 5: Commit Task 2 with its README action documentation.** Message: `feat: preserve cockpit return notes safely`.

## Task 3: Unseen results and questions

**Files:** Modify `continuity.rs`, `app.rs`, `app/continuity_ui.rs`, `tests.rs`, README.

**Interfaces:**
- Add `UpdateKind::{Result, Question}` and `Update { session: String, identity: String, kind: UpdateKind, observed_at: i64, text: String }`.
- Add `Continuity::observe_sessions(&mut self, sessions: &[Session], now: i64) -> Result<bool, String>`, `Continuity::updates(&self) -> Vec<Update>`, and `Continuity::reviewed(&mut self, session: &str, identities: &[String]) -> Result<bool, String>`.
- Add `App::open_updates(&mut self, screen: &mut dyn Screen) -> String`. Pick a specific update using existing fzf/Screen facilities; display its full captured text in the existing scrollable details view, then offer explicit reviewed/open-live-session actions. Acknowledgement uses the displayed identities, not whatever happened to become latest while the detail view was open.
- Result identity is session id + stable turn id. Question identity hashes the full explicit question and kind, ignoring activity timestamp/short display text. Questions require `needs_input=true`; do not infer them from permission-dialog text.

- [ ] **Step 1: Add failing App tests.** Use snapshots containing `last_result` to assert `unseen result` and summary counts; then assert view/focus/open leaves it unseen while palette `reviewed` clears the displayed id. Separate tests cover a new identical-outcome turn id; tick/restart deduplication; new working state retaining unseen completion; question and result from the same Stop; changed/resolved questions; session exit retaining archived results/notes; and old producers displaying `Result tracking unavailable; restart to enable` without inventing unseen results.
- [ ] **Step 2: Run the filtered tests; record the expected failures.**
- [ ] **Step 3: Implement observation and UI.** Observe the full gathered live-session set before filtering; the attention filter must not erase events or clear questions on omitted rows. Resolve absent questions only from an authoritative full snapshot. Persist meaningful observation changes once; no render writes. Reuse SessionDetails by adding a title parameter and the minimum explicit actions needed, keeping existing session-details text/scroll behaviour. Register `updates` and `reviewed`; show separate unseen counts and compact per-card labels without changing activity or ordering. Archived updates must never invoke live terminal actions for missing targets. Display state errors in a durable warning, not a message cleared by the next key. Audit acknowledgements without plaintext questions.
- [ ] **Step 4: Run filtered and full cockpit library tests.** Add a failing-save test asserting the update remains unseen after a failed `reviewed`, and a race test where acknowledging an old displayed result does not mark a later one reviewed. Expected: all pass with stable card selection and layout.
- [ ] **Step 5: Commit with README updates.** Message: `feat: track unseen cockpit results and questions`.

## Task 4: Explain queue eligibility using actual dispatch policy

**Files:** Create `queue.rs`; modify `lib.rs`, `app.rs`, `app/continuity_ui.rs`, `tests.rs`, README.

**Interfaces:**
- Add `QueueContext { now: i64, enabled: bool, gaming: bool, viewed: String, workspace_active: bool }` and `QueueBlock::{Disabled, Gaming, External, Terminal, MissingDirectory, Unknown, Working, NeedsInput, CommandRunning, Busy, PendingMessage, Viewed, Idle { remaining: i64 }}`.
- Add `queue::block(session: &Session, context: &QueueContext, busy: bool, has_pending_message: bool) -> Option<QueueBlock>` and `queue::candidates(session: &Session, todos: &TodoList, issues: &[Issue]) -> Vec<Entry>`.
- Use the existing queue minimum `300` seconds and check interval `30` seconds; name them once for dispatch and explanation. Preserve `todo::queue_order` and design-role routing.
- Add `App::open_queue(&mut self) -> String` for scrollable project/all-project queue details. It consumes current gathered context, not new GitHub/network calls.

- [ ] **Step 1: Add failing parity tests.** For disabled queue, gaming, external target, terminal session, unknown state, working, explicit question on `waiting`, approval/input, running command, busy assignment, viewed active session and idle at 299/300 seconds, assert both the displayed reason and whether `claude-sessions say` was called. Parametrise these cases. Assert an eligible explanation says `ready for the next dispatch check`, never a promised exact start time. Check shared ordering and separate designer/developer candidates with the existing multi-session queue fixtures.
- [ ] **Step 2: Run new cases plus existing `queue` tests; record the expected failures.**
- [ ] **Step 3: Extract and reuse the policy without changing dispatch scheduling.** `dispatch_queue` reevaluates candidates after each assignment, preserving one distinct item per session and no duplicate dispatch. Any `needs_attention()` state, running approved command, or pending manual message is unavailable for automatic new todo work; manual messages retain their own existing send path and order. Report `PendingMessage` distinctly rather than disguising it as Busy. The queue view groups actual next candidates by project and role and explains unavailable issue data; no new actions dispatch work from the view.
- [ ] **Step 4: Run queue and full library suites.** Add regression tests showing a deferred/explicit-question `waiting` session receives no todo and a pending manual message is not displaced by auto dispatch. Assert explanation rendering makes no `git`/`gh` calls and no changes to todo state.
- [ ] **Step 5: Commit with README queue documentation.** Message: `feat: explain cockpit queue decisions with shared policy`.

## Task 5: Navigate a task through its sessions, results and PRs

**Files:** Modify `continuity.rs`, `app.rs`, `app/continuity_ui.rs`, `todo.rs`, `reviews.rs`, `tests.rs`, affected todo command tests and README.

**Interfaces:**
- Add `Review.head_branch: String`, `Review.head_oid: String` and preserve `headRefName`; request `headRefOid` in the existing fetch. Add `Reviews::cached(&self) -> Vec<Review>` returning deduplicated cached rows across scopes with fetch availability represented explicitly; never start a fetch from render.
- Add `TaskSnapshot { key: String, repository: String, title: String, notes: String, status: String }` and `TaskAssignment { task: String, session: String, assigned_at: i64, branch: String, repository: String, baseline_result: Option<String> }`.
- Add `Continuity::assign(&mut self, task: &TaskSnapshot, session: &Session, now: i64) -> Result<bool, String>` and `Continuity::observe_tasks(&mut self, todos: &TodoList, sessions: &[Session], reviews: &[Review], now: i64) -> Result<bool, String>`.
- Add `TrailTarget::{Task(String), Session(String), PullRequest { repository: String, number: i64 }}` and `App::open_task_trail(&mut self, target: TrailTarget, screen: &mut dyn Screen) -> String`. Each trail presents related choices before navigating; no arbitrary first match.

- [ ] **Step 1: Add failing linkage tests.** Prove direct h/queue assignment, delayed background assignment via an attributed `claude-todo start`, exact task-branch matching and full issue-repository matching. Prove same-named repos/branches/titles never link without canonical identity. Prove one task with multiple sessions and one session reused for multiple tasks preserve separate assignments; an already-present `baseline_result` is not assigned to the new task. Prove PR links require exact repository+case-sensitive head branch, preserve every match, and retain last-known identity after disappearance without claiming merge.
- [ ] **Step 2: Run new tests and existing todo/handoff/reviews cases; record the expected failures.**
- [ ] **Step 3: Implement linking and real navigation.** Existing todo handoff text must include `claude-todo start <key>` before work and retain the exact `(todo <key>: …)` recognition shape. Known-id successful handoffs/dispatches call `assign`; id-less launches show `Starting; awaiting assignment` until an attributed update/valid exact branch establishes identity. Snapshot titles/notes before Todo pruning removes them.

  Reconcile the previous and incoming active assignments before attaching results. If an assignment changes and a new result first appears in the same observation, the order is ambiguous: retain that result as a session update, but do not attribute it to either task. Establish the incoming assignment's baseline as the already-present `last_result.id`; attach only a later unambiguously observed completion. Similarly, first observation after restart must not backfill a task result from current titles or timestamps alone. A task trail says `No result recorded` when evidence is insufficient. This conservative rule avoids a false A/B association even when both events share the same Unix second; taking task B must never rewrite A's recorded result.

  Preserve PR head metadata from all fetch paths and differentiate loading/fetch-failed/unavailable from confirmed empty cached results. Track exact confirmed PR identities but do not permanently infer PR↔new-task linkage solely from a reused branch: use the assignment's known branch/repository and display any ambiguity as choices. For old sessions with no canonical metadata, show missing identity instead of host Git discovery.

  Register `task-trail` for panels 1–3. Live session navigation calls existing `view`; existing task navigation selects the correct Todo tab without status changes; absent/pruned tasks open archived details, not live Todo rows. PR navigation reuses the existing browser action. Exited-session details use recorded metadata; resume only through an explicit action after exact cwd/conversation resolution. No missing target silently starts work.

- [ ] **Step 4: Run filtered, full library and todo integration suites.** Assert actual fake manager/browser calls for every navigation type, choices for multiple matches, no calls for missing/unavailable links, and archive navigation after seven-day Todo pruning. Add an observation-order race test where assignment updates and completion arrive in one tick; do not attach a completion predating assignment or to multiple active tasks accidentally.
- [ ] **Step 5: Commit with README trail documentation.** Message: `feat: connect cockpit tasks to sessions results and pull requests`.

## Task 6: Defer questions and PR reviews safely

**Files:** Modify `continuity.rs`, `app.rs`, `app/continuity_ui.rs`, `tests.rs`, README.

**Interfaces:**
- Add `DeferralTarget::{Question { session: String, fingerprint: String }, PullRequest { repository: String, number: i64, head_oid: String, decision: String }}` and `Deferral { target: DeferralTarget, expires_at: i64 }`.
- Add `Continuity::defer(&mut self, target: &DeferralTarget, now: i64) -> Result<bool, String>`, `Continuity::bring_back(&mut self, target: &DeferralTarget) -> Result<bool, String>`, and `Continuity::is_deferred(&self, target: &DeferralTarget, now: i64) -> bool`.
- Add `App::attention_indices(&self, now: i64) -> Vec<usize>` for the `n` action and `f` filter; consume full live-session observations and the same fingerprint validation, not copied per-call predicates.
- `expires_at = now + 1200`. Persist absolute times; reject malformed times. Deferral does not mark any update reviewed, change todo status or suppress approval/banner/phone mechanisms.

- [ ] **Step 1: Add failing real action tests.** A current explicit question can be deferred through `defer-20m`; normal card stays visible with expiry, deferred count increments, and `n`/`f` omit it while keeping other attention. At `now+1199` it is deferred; at `now+1200` it is available again. Test restart, overdue-at-launch, explicit bring-back, changed full question, and changed PR head/decision. Question state remains blocked throughout.
- [ ] **Step 2: Run the new tests; record the expected failures.**
- [ ] **Step 3: Implement the exact-target deferral and shared navigation.** Add palette actions scoped to Sessions and PRs; only explicit questions and PRs requiring review are eligible. A meaningful revision/new request removes the prior deferral immediately; absent PR data does not prove a revision or erase the retained record. Approval/permission/command-input or unknown states override a question deferral even if the old `needs_input` flag remains. Expiry affects the next cockpit refresh, without promising background notifications. Keep unfiltered attention/deferred/unseen counts separate from filtered visible sessions.
- [ ] **Step 4: Run the full cockpit library suite.** Parametrise refusal tests for permission dialogs, approval, command input, command running, unknown, Ready and PRs not needing review; unknown head revision cannot be deferred. Assert no manager `say`/permission/approval calls, no lost queued work, and no change to existing notification outputs. Verify a command request invalidates the old question deferral even when the same question text later returns. Failed save must leave the item immediately actionable with a visible error.
- [ ] **Step 5: Commit with README examples and the explicit cockpit-only scope.** Message: `feat: defer cockpit questions and reviews without approving them`.

## Final verification and delivery

- [ ] Run `git diff --check` and inspect the complete branch diff; no unintended manifest, lockfile, launcher, Makefile, compose, screenshot or generated preview files staged.
- [ ] With explicit build/test permission, run the owning suites and full workspace commands through `agent-build`: `cargo test --manifest-path apps/Cargo.toml -p cockpit`, `cargo test --manifest-path apps/Cargo.toml -p desktop --lib`, then the existing `make test` (which also builds the OSD test executable). Record verbatim output and do not disable or baseline failures.
- [ ] Check all six tasks against the approved spec. In particular: archived updates/notes, corrupt state errors surviving key presses, task snapshots after pruning, no render-time writes/network calls, and unchanged shortcuts/attention ordering. Use existing fake Screen/buffer tests for short/narrow Unicode layouts; browser HTML is not proof that the terminal works.
- [ ] Preserve the execution method Theo chose at plan approval. Native means the implementer runs every task in this session; after implementation a fresh read-only reviewer checks the whole branch as authorised by the selected execution workflow. Subagent-driven requires loading its available implementation skill before dispatch; if unavailable, report that rather than inventing a workflow.
- [ ] Report diff stat, what changed/what stayed, and verbatim test outcomes. State explicitly that `session-state.sh` and the desktop daemon participate in host execution, and that existing sessions need a restart to load reporting changes.
- [ ] On the authorised feature branch, commit only requested completed work. Do not rebase, merge, push, change the default branch or touch unrelated `.superpowers/` preview files. If current upstream divergence prevents a safe apply, ask Theo instead of rewriting history.
- [ ] Export with `git format-patch origin/main -o .outgoing`. Through `ask-to-run`, request `claude-sessions apply-dotfiles /home/theo/IdeaProjects/.parallel/dotfiles/todo-135a843b`, then the repository-required host `make -C ~/dotfiles cockpit test` and `claude-sessions reload`. Preserve host diff/approval; a declined apply is not a successful deployment.
- [ ] Once implementation and its required checks/apply finish, mark todo `135a843b` done. Its brainstorming portion was already completed; do not claim implementation complete early. Offer to stop the temporary mockup service; do not leave it running indefinitely or remove the user's browser tab.

## Plan self-review

- Coverage: protocol prerequisites (1), state/notes (2), unseen events (3), queue (4), trails (5), deferral (6), and delivery above cover every spec section.
- All later tasks use the fields and method names defined in preceding Interfaces blocks; PR revision is introduced in Task 5 before Task 6 uses it.
- Review Focus conditions are pinned by the corresponding task's tests, including event/assignment races and blocked queue capacity.
- New type/function names in Interfaces are proposed implementation names, not claims about existing APIs. Existing entrypoints and fixture commands were verified in this checkout.
- No code, tests, builds, dependency changes or live deployment were performed while writing this plan.
