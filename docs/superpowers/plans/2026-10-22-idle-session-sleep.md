# Idle Session Sleep Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Exit Claude and opencode sessions idle at their prompt for an hour, keep their cards as `Asleep`, and resume the same conversation whenever the card is opened, messaged, given a todo or woken from the phone.

**Architecture:** The column decides who may sleep (pure `sleep::block`, like `queue::block`) and calls `cockpit sleep <id>` in the background. The script does the last-moment pane checks, writes a durable sleep note, and exits the session through the agent's exit (never `cockpit exit`). A wake writes the existing restart note and runs the agent's detached restore, so the column's `Restarting` card and the viewer's restarting screen are reused unchanged. Messages to an asleep session go to the existing prompt queue and are delivered by `dispatch_prompts` once it is back.

**Tech Stack:** Rust (apps/cockpit), bash (`config/bin/.local/bin/cockpit`, `cockpit-agents/*.sh`), jq, the phone page (`apps/cockpit/assets/remote/page.js`).

**Spec:** `docs/superpowers/specs/2026-10-22-idle-session-sleep-design.md`

## Global Constraints

- Sleep after `idle-sleep-after` minutes, default **60**, minimum **30**; switch `idle-sleep`, default on.
- Sleep notes: `~/.local/state/cockpit/asleep/<session-id>` (state dir, survives reboots), JSON, mode 0600.
- An asleep note older than **7 days** (`slept_at`) is deleted; the card disappears; `H` still lists the session.
- A sleep never appends to `ended-sessions`. Only `x` (`cockpit exit`) does.
- Never sleep: `terminal`, external, pinned, viewed (`cockpit.current`), team members, a session with a queued prompt or a todo in progress, any card not `Ready`, any agent without the `sleep` feature (Codex).
- Any doubt in a check means stay awake, and the reason is logged (`log_event INFO sleep "<project> stays awake: <reason>"`).
- A wake never starts a fresh empty session in place of a missing conversation.
- Commit messages end with `Todo: c17a056c` only on the last task's commit.

## Review Focus

1. A Claude prompt showing its dim placeholder after `❯` must read as empty, while a real draft (including one whose cursor sits at its start) must read as a draft: Task 2 tests both.
2. A message, todo or phone message sent while the session is waking (restart note present, no pane yet) must be queued, not refused: Task 3 `say_while_waking_queues_the_message`.
3. A stale sleep note for a session that is running again (resumed with `H`, or restored by hand) must not produce a second card: Task 4 `a_running_session_drops_its_stale_sleep_note`.
4. After a reboot, asleep sessions must not appear in the `B` restore offer: Task 4 `asleep_sessions_are_not_offered_for_restore`.
5. A sleep that fails midway (exit refused, pane gone) leaves no note behind and the session untouched: Task 2 `a_failed_exit_removes_the_sleep_note`.

---

### Task 1: The sleep model in Rust

**Files:**

- Create: `apps/cockpit/src/sleep.rs`
- Modify: `apps/cockpit/src/lib.rs` (add `pub mod sleep;`), `apps/cockpit/src/paths.rs` (add `asleep_dir`), `apps/cockpit/src/agents.rs` (add `Feature::Sleep` to Claude and opencode, and its name in `feature_names`), `apps/cockpit/src/sessions.rs` (`Session.asleep`, `is_external`, `is_idle`)
- Test: unit tests in `apps/cockpit/src/sleep.rs` (`#[cfg(test)] mod tests`, as sessions.rs does)

**Interfaces:**

- Produces:
  - `paths::asleep_dir() -> PathBuf` = `state_dir().join("asleep")`
  - `sessions::Session.asleep: Option<i64>`: when it fell asleep; `None` for a live session
  - `Session::is_external()` is false when `asleep.is_some()`; `sessions::is_idle` is false for an asleep card
  - `pub struct Asleep { pub session: String, pub agent: String, pub project: String, pub cwd: String, pub title: String, pub idle_since: i64, pub slept_at: i64, pub remote: bool }`
  - `pub fn asleep() -> Vec<Asleep>` reads every note, skipping unreadable ones with a WARN log
  - `impl Asleep { pub fn card(&self) -> Session }`: `state: "waiting"`, `since: idle_since`, `asleep: Some(slept_at)`, `task: title`, other fields from the note
  - `pub fn forget(session: &str)` deletes a note
  - `pub const EXPIRY_S: i64 = 7 * 86_400;` `pub fn expire(now: i64) -> Vec<Asleep>` deletes and returns notes with `now - slept_at >= EXPIRY_S`
  - `pub const DEFAULT_AFTER_MIN: i64 = 60; pub const MIN_AFTER_MIN: i64 = 30;` `pub fn after_minutes() -> i64` reads `settings::launch_value("idle-sleep-after")`: empty means 60; unparsable or below 30 logs a WARN and returns 60
  - `pub struct SleepContext { pub now: i64, pub enabled: bool, pub after_s: i64, pub viewed: String, pub team_members: HashSet<String> }`
  - `pub enum SleepBlock { Disabled, Asleep, Restarting, External, Terminal, Unsupported, Pinned, Viewed, Team, NotReady, Busy, PendingMessage, Idle { remaining: i64 } }` with a `Display` like `QueueBlock`'s
  - `pub fn block(session: &Session, context: &SleepContext, busy: bool, has_pending_message: bool) -> Option<SleepBlock>`, checked in the enum's order; `NotReady` = `activity_label(session) != "Ready"`; `Unsupported` = `!session.agent().can(Feature::Sleep)`

- [ ] **Step 1: Write the failing tests** in `sleep.rs`:
  - `block_allows_a_ready_session_idle_past_the_delay`: a waiting Claude session, `since = now - 3600`, `after_s = 3600` → `None`.
  - `block_reasons` (one table, parametrised): each of disabled, asleep, restarting, external (no pane, no kitty), terminal, Codex agent, pinned, viewed, team member, `state == "working"`, `needs_input`, `ask_to_run == "approval"`, busy, pending message → the matching `SleepBlock`. Idle at 3599 s → `Idle { remaining: 1 }`.
  - `notes_round_trip_into_cards`: write a note JSON into a temporary `asleep_dir` (set `XDG_STATE_HOME`/`COCKPIT_STATE_HOME` the way existing path tests do) → `asleep()[0].card()` has `asleep == Some(slept_at)`, `since == idle_since`, `!is_external()`, and `!sessions::is_idle(...)`.
  - `expire_drops_notes_a_week_old`: notes at 7 days and at 6 days 23 h → only the first is removed and returned.
  - `after_minutes_defaults_and_floors`: unset → 60, `"45"` → 45, `"10"` → 60, `"abc"` → 60.
- [ ] **Step 2: Run them and see them fail.** Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --lib sleep::`. Expected: compile errors (module missing).
- [ ] **Step 3: Implement** the interfaces above. Deserialize notes with `serde_json::Value`, as `Session::from_value` does.
- [ ] **Step 4: Run them and see them pass.** Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --lib sleep::`, then `cargo test --manifest-path apps/Cargo.toml -p cockpit --lib`. Expected: all pass. The second run checks that `is_external` and `is_idle` changed nothing else.
- [ ] **Step 5: Commit** `Model asleep sessions and who may sleep`.

### Task 2: `cockpit sleep <id>`

**Files:**

- Modify: `config/bin/.local/bin/cockpit` (new `sleep_session`, `prompt_has_draft`, `background_shell_of`, dispatch `sleep)`, `KNOWN_SETTINGS += idle-sleep`, `LAUNCH_SETTINGS += idle-sleep-after`, its `validate_launch_setting` case and fallback `""`, the usage comment near line 79), `config/bin/.local/bin/cockpit-agents/claude.sh` and `opencode.sh` (`sleep` in `agent_*_can`), `config/bin/.local/bin/cockpit-agents/README.md` (the `sleep` feature)
- Test: `apps/cockpit/tests/cockpit_script.rs`

**Interfaces:**

- Consumes: `paths::asleep_dir` layout from Task 1 (`$COCKPIT_STATE_HOME`/`$XDG_STATE_HOME` + `/cockpit/asleep/<id>`).
- Produces:
  - `ASLEEP_DIR` in the script.
  - `cockpit sleep <id>`: exit 0 = asleep. Exit 3 = stayed awake, with the reason on stderr and in the log. Exit 1 = error.
  - The note is written with jq: `{session, agent, project, cwd, title, idle_since, slept_at, remote}`. `title` is the snapshot's `task`, `idle_since` its `since`, `remote` comes from `session_is_remote` (Claude only).
  - `cockpit setting idle-sleep-after <minutes>` accepts integers of 30 or more.

`sleep_session` does these steps in order, stopping at the first one that fails:

1. Check the id with `^[A-Za-z0-9_-]+$` and require the `sleep` feature.
2. Require a pane, state `waiting`, and not `needs_input`.
3. Claude: `has_conversation`, and the snapshot's `background` and `agents` are both 0.
4. `prompt_has_draft pane` is false.
5. `background_shell_of pane` prints nothing.
6. Write the note to `$id.tmp` and `mv` it into place.
7. Run `exit_session id`. On failure, `rm` the note and return 1.
8. Run `clear_exited_viewer id` and log `INFO sleep "<project> asleep after <n> min idle"`.

The two pane checks:

- **`prompt_has_draft <pane>`**: false only when the cursor sits right after the agent's `prompt_start` (as in `cockpit-left`) **and** the rest of that line after the cursor is blank or dim. Read the line with `capture-pane -p -e -N`; dim means every visible character after the cursor is inside SGR 2 or SGR 90 (bright black). Anything else, or a tmux error, counts as a draft.
- **`background_shell_of <pane>`**: prints `pid comm` of the first process whose `comm` is in `bash sh zsh fish dash` and whose parent's `comm` is `claude` or `opencode`, among the pane pid's descendants. Use `ps -eo pid=,ppid=,comm=` and the descendant walk from `record_stuck_exit`.

- [ ] **Step 1: Write the failing tests** in `cockpit_script.rs` (fake tmux, `SHIM_PANE_TEXT`, a `ps` shim):
  - `sleep_writes_a_note_and_exits_without_ending_the_session`: exit 0. The note exists with `idle_since` equal to the snapshot's `since`. `/exit` is sent. `ended-sessions` does not contain the id.
  - `sleep_keeps_a_session_with_a_draft`: the pane line is `❯ fix the tests` with the cursor after `❯` and its trailing space → exit 3, no note, no send-keys.
  - `sleep_reads_a_dim_placeholder_as_an_empty_prompt`: the line is `❯ \e[2mTry "refactor"\e[0m` with the cursor after `❯` and its trailing space → exit 0.
  - `sleep_keeps_a_session_with_a_background_shell`: the `ps` shim gives the pane pid → fish → claude → bash → exit 3, and the log names `bash`.
  - `sleep_keeps_a_claude_session_with_background_work`: snapshot `background: 1` → exit 3.
  - `sleep_keeps_a_claude_session_with_no_conversation`: no transcript → exit 3.
  - `sleep_refuses_codex`: exit 1 with "not Codex CLI's to do", as for other features.
  - `a_failed_exit_removes_the_sleep_note`: make `agent_claude_exit` fail (`SHIM_PANE_GONE`, plus a tmux shim that fails `send-keys`) → non-zero, and no note remains.
  - `idle_sleep_after_takes_minutes_of_thirty_or_more` (parametrised): `30` and `90` are saved; `29`, `abc` and `-5` are refused with "invalid idle-sleep-after".
- [ ] **Step 2: Run them and see them fail.** Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test cockpit_script sleep_`. Expected: fails ("unknown command" or exit 1).
- [ ] **Step 3: Implement** the interfaces above.
- [ ] **Step 4: Run them and see them pass.** Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test cockpit_script`. Expected: all pass. Fix any existing test that asserts on the known-settings list or the agent verbs.
- [ ] **Step 5: Commit** `cockpit sleep: exit an idle session and keep a note to wake it`.

### Task 3: `cockpit wake <id>` and every way in

**Files:**

- Modify: `config/bin/.local/bin/cockpit` (`wake_session`, `is_asleep`, `queue_for_wake`; branches in `view_session`, `say_to_session`, `restart_session`, the `exit)` dispatch; dispatch `wake)`)
- Test: `apps/cockpit/tests/cockpit_script.rs`

**Interfaces:**

- Consumes: the note format from Task 2. `RESTARTING_DIR` and its note format (`since\nproject\ndir\nold-pane`, old pane empty). `agent_<agent>_restore dir id`.
- Produces:
  - `cockpit wake <id>`. Exit 0 when the resume has started. Exit 1 when it has not, after deleting the note and logging a WARN with the reason.
  - `is_asleep <id>`: the note exists.
  - `queue_for_wake <id> <message>` writes the message as `queue_prompt_ask` does (`prompt-queue/<id>/<ns>.txt`).

How `wake_session` works:

- If the cwd is missing, or for Claude `has_conversation` fails: delete the note, log `WARN sleep "<project> could not wake: <reason>"`, and return 1.
- Otherwise:
  1. Write the restart note.
  2. If the session is on screen (`$CURRENT`), call `show_restarting`.
  3. Run `agent_<agent>_restore cwd id`. On failure, delete the restart note and return 1.
  4. Delete the sleep note and log `INFO sleep "<project> woke"`.
  5. If `remote` was true, start in the background (`setsid -f`) a wait of at most `RESTART_TIMEOUT_S`. It waits for the snapshot to show the id with a pane, state `waiting` and `started` at or after the wake. Then, unless `session_is_remote`, it calls `send_remote_toggle pane`.

The branches, each placed right where the pane lookup fails today:

- `view_session`: if asleep, wake, `printf '%s\n' "$id" > "$CURRENT"`, `show_restarting`, return 0.
- `say_to_session`: if asleep, or a restart note exists for the id with no pane yet: check the message as today, `queue_for_wake`, then `setsid -f cockpit wake` when asleep. Return 0 with `queued until it wakes` on stdout.
- `restart_session`: if asleep, call `wake_session` and return its status.
- `exit)`: if asleep, delete the note, log it, and keep the existing `ended-sessions` append. Skip `exit_session`.

- [ ] **Step 1: Write the failing tests:**
  - `wake_restores_the_conversation_and_drops_the_note`: Claude, transcript present → `claude --resume <id>` in a detached `claude-retry-restart-*` session, a restart note exists, and the sleep note is gone.
  - `wake_never_starts_a_fresh_session_when_the_conversation_is_gone`: no transcript → exit 1, no `new-session` call, the note is gone, and the log says "could not wake".
  - `wake_turns_remote_control_back_on`: note `remote: true`, then the snapshot shows the id back at its prompt → `/rc` is sent once.
  - `opening_an_asleep_session_wakes_it`: `cockpit open <id>` → restore called, `$CURRENT` holds the id, the viewer respawns `restarting-screen`.
  - `say_to_an_asleep_session_queues_and_wakes`: the message is in `prompt-queue/<id>/`, exit 0, and a wake has started.
  - `say_while_waking_queues_the_message`: a restart note and no pane → queued, exit 0, no second wake.
  - `restart_on_an_asleep_session_wakes_it` and `exit_on_an_asleep_session_forgets_it_for_good` (the note is gone and the id is in `ended-sessions`).
- [ ] **Step 2: Run them and see them fail.** Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test cockpit_script wake_ asleep`. Expected: fail.
- [ ] **Step 3: Implement** the interfaces above.
- [ ] **Step 4: Run them and see them pass.** Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test cockpit_script`. Expected: all pass.
- [ ] **Step 5: Commit** `cockpit wake: resume an asleep session when it is opened, messaged or restarted`.

### Task 4: The column puts sessions to sleep and shows them asleep

**Files:**

- Modify: `apps/cockpit/src/app.rs`:
  - `build_sessions`: add asleep cards after `hold_restarting_cards`.
  - The card badge near line 1529.
  - `session_summary_lines`: an `asleep` bucket. Keep the edit minimal, because todo 353233c3 is changing this function.
  - A new `dispatch_sleep` in the tick, after `dispatch_prompts`.
  - The S menu: a `toggle_setting` row, and a `choose_launch_setting` branch for `idle-sleep-after`.
- Modify: `apps/cockpit/src/settings.rs` (`SETTINGS += ("idle-sleep", "Sleep idle sessions")`, `LAUNCH_SETTINGS += ("idle-sleep-after", "Sleep after (minutes)")`)
- Test: `apps/cockpit/src/tests.rs`

**Interfaces:**

- Consumes: everything from Task 1. `cockpit sleep` and `cockpit wake` from Tasks 2–3.
- Produces:
  - `const SLEEP_CHECK_S: f64 = 60.0;` and an `App.sleep_checked: f64` field.
  - `fn add_asleep_cards(&mut self, running: &mut Vec<Session>)` handles each `sleep::asleep()` note:
    - The session is running with `started >= slept_at`: `sleep::forget` it (a stale note) and add no card.
    - It has a restart note: skip it (the restarting card shows instead).
    - Otherwise: push `note.card()`.
  - `pub fn dispatch_sleep(&mut self)` runs once per `SLEEP_CHECK_S`:
    1. Log and drop expired notes (`sleep::expire`).
    2. Build a `SleepContext`: `enabled = settings::is_on("idle-sleep")`, `after_s = sleep::after_minutes() * 60`, `viewed = current_session()`, team members = the keys of `team_roles`.
    3. Call `manager_in_background(&["sleep", id])` for every `running_sessions` entry where `sleep::block(...)` is `None`. `busy` is `self.todos.busy(id)`; `has_pending_message` is `prompts::count(id) > 0`.
  - Badge: `Asleep {util::age(slept_at)}` in `FG_DARK`, dim like Idle.
  - Summary: `N asleep` after idle, counted apart from idle.
  - The `idle-sleep-after` row cycles 30 → 60 → 120 → 240 → 30 on Enter, through `manager(&["setting", "idle-sleep-after", value])`.

- [ ] **Step 1: Write the failing tests** (the `Fixture` style of `ready_sessions_left_alone_past_the_idle_threshold_show_as_idle_not_ready`, notes written into the fixture's state dir):
  - `an_asleep_note_shows_as_an_asleep_card`: the badge starts with `Asleep` followed by a space, and the card is listed with no snapshot row.
  - `a_running_session_drops_its_stale_sleep_note`: snapshot row with `started > slept_at` → one card, and the note is deleted.
  - `asleep_sessions_are_not_offered_for_restore`: `recent-sessions.json` lists the id, with a boot time in the offer window → `app.restore` is empty.
  - `summary_counts_asleep_sessions`: "1 asleep".
  - `asleep_cards_do_not_count_as_started_before_a_change`: CLAUDE.md newer than the note → the outdated warning does not count the asleep card.
  - `dispatch_sleep_sleeps_only_eligible_sessions`: one waiting session idle for 2 h, plus one idle for 10 min → exactly one `cockpit sleep` call, recorded the way existing tests record `manager_in_background` calls.
  - `dispatch_sleep_does_nothing_when_switched_off`: `idle-sleep` set to off → no call.
  - `dispatch_sleep_expires_week_old_notes`: the note is removed.
  - `the_queue_gives_an_asleep_session_its_todo`: an asleep card for the project plus a queued item → `cockpit say <id> …` is called (the script queues and wakes), and the item becomes `doing`.
  - `settings_menu_cycles_the_sleep_delay`: from 60, Enter → `setting idle-sleep-after 120`.
- [ ] **Step 2: Run them and see them fail.** Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --lib asleep sleep`. Expected: fail.
- [ ] **Step 3: Implement** the interfaces above.
- [ ] **Step 4: Run them and see them pass.** Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --lib`. Expected: all pass.
- [ ] **Step 5: Commit** `Column: put idle sessions to sleep and keep their cards`.

### Task 5: The phone page sees and wakes asleep sessions

**Files:**

- Modify: `apps/cockpit/src/remote.rs` (`state`, `message`, new `wake`), `apps/cockpit/src/bin/claude-remote.rs` (route `POST /api/wake/<id>`), `apps/cockpit/assets/remote/page.js` (the asleep state and a Wake button)
- Test: `apps/cockpit/tests/claude_remote.rs`, and `apps/cockpit/tests/remote_mobile_ui.mjs` if the page's rendering tests live there

**Interfaces:**

- Consumes: `sleep::asleep()` and `Asleep::card()`.
- Produces:
  - `remote::state()` appends one card per asleep note not in the snapshot: `"state": "asleep"` and `"since": slept_at`, with the other fields as today.
  - `remote::message(id, text)` on an asleep id: `prompts::add`, then `util::manager_in_background(&["wake", id])` → `Ok("queued; it wakes now")`.
  - `pub fn wake(id: &str) -> Result<String, String>`: `"no such session"` unless a note exists; otherwise a background `cockpit wake` → `Ok("waking")`. Audit it as `remote.wake`.
  - Route: `(Method::Post, ["api", "wake", id]) if is_session_id(id)`, as the `interrupt` route does.
  - Page: an asleep card shows `asleep · <age>` and a Wake button. Its yes/no, interrupt and remote-control buttons are hidden.

- [ ] **Step 1: Write the failing tests:**
  - `phone_lists_asleep_sessions`: a note with no snapshot row → `/api/state` includes the id with `state: "asleep"`.
  - `phone_wake_starts_cockpit_wake`: the fake `cockpit` records `wake <id>`.
  - `phone_message_to_an_asleep_session_is_queued_and_wakes_it`.
  - `phone_wake_refuses_an_unknown_session`.
  - A UI test: an asleep card renders a Wake button.
- [ ] **Step 2: Run them and see them fail.** Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test claude_remote asleep wake`. Expected: fail.
- [ ] **Step 3: Implement** the interfaces above.
- [ ] **Step 4: Run them and see them pass.** Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test claude_remote` and the page's UI test command, as `make test` runs it. Expected: all pass.
- [ ] **Step 5: Commit** `Phone page: list asleep sessions and wake them`.

### Task 6: Clone sweep, docs, full suite

**Files:**

- Modify: `config/bin/.local/bin/cockpit` (`clone_leftovers` / `reviewed_clone_report`: a clone whose path is an asleep note's `cwd` counts as having a session), `README.md` (Sessions section: the Asleep badge, wake triggers, the two settings and the 7-day expiry; the settings list in the `S` sentence)
- Test: `apps/cockpit/tests/cockpit_script.rs`

- [ ] **Step 1: Write the failing test** `clone_sweep_keeps_a_clone_with_an_asleep_session`: an asleep note's cwd is a clean clone → it is not offered for deletion.
- [ ] **Step 2: Run it and see it fail.** Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test cockpit_script clone_sweep_keeps`. Expected: fail.
- [ ] **Step 3: Implement** the check and update `README.md`.
- [ ] **Step 4: Run the whole suite.** Run `make test`. Expected: everything passes. Report any failure word for word; known-flaky tests (todos 01765090, a4da8137) are named as such, never skipped.
- [ ] **Step 5: Commit** `Keep clones of asleep sessions; document idle sleep`, with the trailer `Todo: c17a056c`.
