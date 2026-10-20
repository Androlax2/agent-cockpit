# Agent-requested teams Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An agent asks for missing teammates once; after Theo's yes the cockpit starts them (Designer/Developer at approval, QA per handoff), links them, and gives each its brief.

**Architecture:** A new agent action (`recruit`) creates a pending proposal that also carries recruits, briefs and the default agent read at request time. Approval writes spawn rows; the daemon starts them through a new `claude-sessions team-spawn` command, passing a one-time token in the launch environment. The daemon binds a reporting session to its spawn by reading that token from the launch's host process, never from the session's report. A handoff in a team with a standing QA brief makes a QA spawn in a fresh copy at the commit, and the previous QA session is exited.

**Tech Stack:** Rust (crates `cockpit`, `desktop`), rusqlite, bash (`claude-sessions`), TypeScript only for the existing OpenCode plugin (unchanged).

**Spec:** `docs/superpowers/specs/2026-10-21-agent-requested-teams-design.md` (builds on `2026-10-20-linked-cockpit-sessions-design.md`).

## Global Constraints

- Nothing starts before Theo's yes; agents can never approve, edit or widen a request.
- New teammates run in the global default agent (`claude-sessions` setting `default-agent`), read when the request is created, shown on it, fixed by approval.
- Designer and Developer start in the asker's folder; QA starts per handoff in a fresh copy at exactly the handed-off commit.
- Spawn identity comes only from the launch's host process environment (`CLAUDE_SESSIONS_TEAM_SPAWN`, 32 lowercase hex), single use.
- A previous QA session is unlinked and exited through `claude-sessions exit` when a newer handoff starts the next QA.
- Briefs: 1–2000 characters, no control characters except newline, shown escaped; a brief is labelled "written by <asker role>, approved by Theo" in the new session's first message.
- Team limits unchanged: one Designer, one Developer, one QA; a session belongs to one team.
- Daemon never blocks on a start: session starts are spawned and not waited for.

## Review Focus

1. A recruit request while the asker already belongs to a team → only that team's missing roles may be recruited, and the asker's role must equal its current one (store test in Task 1).
2. A spawned session that never reports → its spawn turns `failed` after 5 minutes and shows in the Team view as needing Theo; nothing is started again automatically (Task 2, Task 6).
3. A forged token: a session whose report claims a spawn, or a launch environment token that is unknown/used/expired → no membership (Task 4).
4. Two handoffs in quick succession → exactly one QA spawn per handoff, the older QA spawn not yet started is cancelled, and only the newest QA becomes a member (Task 3).
5. A default agent that cannot run (setting unreadable) → the recruit request is refused with a clear error, not created with a guessed agent (Task 4).

---

### Task 1: Recruit requests in the protocol and the store

**Files:**
- Modify: `apps/cockpit/src/teams.rs` (types), `apps/cockpit/src/teams/store.rs` (schema v2, `recruit`, mapping hash)
- Modify: `apps/desktop/src/teams.rs` (dispatch the new action), `apps/desktop/src/registry.rs` (`System::default_agent`), `apps/cockpit/src/bin/cockpit-team.rs` (action name)
- Test (also): `apps/desktop/src/tests.rs` — `an_unreadable_default_agent_refuses_the_recruit_request` moves here from Task 4
- Test: `apps/cockpit/tests/team_store.rs`, `apps/cockpit/tests/team_protocol.rs`

**Interfaces:**
- Produces:
  - `pub struct Recruit { pub role: Role, pub brief: String }` (serde `deny_unknown_fields`)
  - `pub struct Recruitment { pub submission_key: String, pub task: TaskKey, pub role: Role, pub recruits: Vec<Recruit>, pub reason: String }`
  - `AgentAction::Recruit { recruitment: Recruitment }`
  - `store::recruit(connection: &mut Connection, sender: &VerifiedSession, recruitment: &Recruitment, agent: &str, live: &[VerifiedSession], now_ms: i64) -> Result<TeamReply, TeamError>`; reply `{"proposal_id","revision":1,"mapping_hash"}`
  - proposals table gains `recruits TEXT NOT NULL DEFAULT '[]'` and `spawn_agent TEXT NOT NULL DEFAULT ''` (migration: `user_version` 1 → 2 via `ALTER TABLE`; open accepts 0..=2)
  - mapping hash for a recruit proposal = sha256 of `"{task_key}\n{mapping}\n{recruits_json}\n{spawn_agent}"`; plain proposals keep today's hash.

- [ ] **Step 1: Write the failing tests** in `team_store.rs`:
  - `a_recruit_request_stays_pending_and_binds_only_the_asker` — `recruit(...)` with asker role Developer, recruits Designer+Qa, agent `"claude"` → snapshot proposal `status == "pending"`, mapping has exactly the asker as developer, `recruits` lists designer and qa with briefs, `spawn_agent == "claude"`; members empty.
  - `recruit_validation_refuses_bad_requests` — each refused: recruit role equal to asker role; duplicate recruit roles; empty or >2000-char brief; brief with `\u{1b}`; zero recruits; agent `""`.
  - `an_asker_in_a_team_may_only_fill_missing_roles` — linked team (designer, developer, qa from `linked()`) → recruiting a Designer from the developer is refused; after unlinking the designer it is accepted; asker role different from its member role is refused.
  - `changing_recruits_or_agent_changes_the_mapping_hash` — two requests differing only in a brief (or only in agent) have different `mapping_hash`.
  - `an_existing_version_1_store_opens_and_gains_recruit_columns` — create a v1 file via the old schema SQL (copy the v1 `CREATE TABLE proposals` statement into the test), open with `store::open`, `PRAGMA user_version` is 2.
  - In `team_protocol.rs`: `recruit_actions_reject_unknown_fields` — JSON with an extra field fails to deserialize.
- [ ] **Step 2: Run** `cd apps && cargo test --offline -p cockpit --test team_store --test team_protocol` — expect compile errors for missing types.
- [ ] **Step 3: Implement** the types, migration, validation (reuse `bind` for the asker alone; reuse the existing per-sender rate limit and submission-key idempotency of `propose`), and the hash. Add `System::default_agent(&mut self) -> Option<String>` in `apps/desktop/src/registry.rs` (reads `$XDG_STATE_HOME/claude-sessions/settings.json` key `default-agent`; `"claude"` when the file is absent; `None` when unreadable or not a name in `cockpit::agents`), and dispatch `AgentAction::Recruit` in `apps/desktop/src/teams.rs::request` with it, refusing with `Unavailable` "cannot read the default agent" on `None`. Add `"recruit"` to the accepted names in `cockpit-team.rs`.
- [ ] **Step 4: Run** the same command — all pass; run `cargo test --offline -p desktop --lib` — pass.
- [ ] **Step 5: Commit** `git commit -m "Add recruit requests: an agent asks for missing teammates"`

### Task 2: Approval writes spawns; spawns bind once and expire

**Files:**
- Modify: `apps/cockpit/src/teams/store.rs`
- Test: `apps/cockpit/tests/team_store.rs`

**Interfaces:**
- Consumes: Task 1's proposal columns.
- Produces:
  - table `spawns (token TEXT PRIMARY KEY, task_key TEXT NOT NULL, role TEXT NOT NULL, agent TEXT NOT NULL, directory TEXT NOT NULL, commit_id TEXT, brief TEXT NOT NULL, handoff_id TEXT, status TEXT NOT NULL, session_id TEXT, created_at INTEGER NOT NULL, started_at INTEGER)`; status ∈ `pending | started | bound | failed | cancelled`
  - table `team_qa (task_key TEXT PRIMARY KEY, brief TEXT NOT NULL, agent TEXT NOT NULL)` — the standing QA brief
  - `pub struct Spawn { pub token: String, pub role: Role, pub agent: String, pub directory: String, pub commit_id: Option<String>, pub brief: String }`
  - `store::pending_spawns(connection: &Connection) -> Result<Vec<Spawn>, TeamError>`
  - `store::mark_spawn_started(connection: &Connection, token: &str, now_ms: i64) -> Result<(), TeamError>`
  - `store::bind_spawn(connection: &mut Connection, token: &str, session: &VerifiedSession, now_ms: i64) -> Result<bool, TeamError>` (true when it made a member)
  - `store::expire_spawns(connection: &Connection, now_ms: i64) -> Result<(), TeamError>` (started > 300_000 ms ago and unbound → `failed`)
  - `store::control` gains a parameter `directories: &HashMap<String, String>` (live session id → its cwd), supplied by the daemon; `HostAction` is unchanged.
  - snapshot gains `"spawns"` (non-cancelled rows, newest 100) and `TeamView::attention` counts `failed` spawns.

- [ ] **Step 1: Write the failing tests:**
  - `approving_a_recruit_request_links_the_asker_and_queues_its_teammates` — approve → asker is a member; `pending_spawns` returns Designer with the asker's directory and brief, agent `"claude"`; no QA spawn; `team_qa` holds the QA brief.
  - `a_spawn_binds_its_first_session_once` — `bind_spawn` with a verified session of the same project → member with role designer, spawn `bound`; a second `bind_spawn` with another session → `Ok(false)`, no change.
  - `a_spawn_refuses_a_session_of_another_project_or_task` — project mismatch → `Ok(false)`; session already member of another task → `Ok(false)`.
  - `an_unknown_or_expired_token_binds_nothing` — unknown token → `Ok(false)`; after `expire_spawns(now + 300_001)` a started spawn is `failed` and `bind_spawn` → `Ok(false)`.
  - `rejecting_or_superseding_a_recruit_request_queues_nothing` — reject → `pending_spawns` empty.
  - `failed_spawns_count_as_needing_theo` — in `team_ui.rs`: snapshot with one `failed` spawn → `TeamView::attention == 1`.
- [ ] **Step 2: Run** `cargo test --offline -p cockpit --test team_store --test team_ui` — fail.
- [ ] **Step 3: Implement.** Approval of a proposal with recruits: insert one spawn row per Designer/Developer recruit (`token = identifier()`, directory = `directories[asker]`, refuse with `Conflict` "the asking session is no longer live" when absent), and upsert `team_qa` when a Qa recruit is present. Binding requires `session.project == task project` and inserts the member like `approve_mapping` does (epoch = `identifier()`), then `schedule_qa`.
- [ ] **Step 4: Run** — pass; `cargo test --offline -p desktop --lib` — pass after updating the `control` call site with an empty map (Task 4 supplies the real one).
- [ ] **Step 5: Commit** `git commit -m "Approved recruit requests queue their teammates; spawns bind once"`

### Task 3: A handoff starts a fresh QA

**Files:**
- Modify: `apps/cockpit/src/teams/store.rs` (`handoff`, `schedule_qa`), `apps/cockpit/src/teams.rs` (`qa_brief_text`)
- Test: `apps/cockpit/tests/team_store.rs`

**Interfaces:**
- Consumes: Task 2 tables.
- Produces:
  - `store::handoff` reply gains `"qa_spawned": bool`.
  - `store::superseded_qa(connection: &Connection) -> Result<Vec<String>, TeamError>` — QA session ids unlinked by a newer handoff and not yet exited; `store::mark_qa_exited(connection, session_id)`.
  - `pub fn qa_brief_text(standing: &str, handoff: &Value, findings: &[String]) -> String` in `teams.rs`: names the commit, branch, checkout, tests, FigCraft frames, the standing brief, the earlier findings (newest 10, each ≤ 500 chars), "do not change product code", and the reply command shape for findings (as `envelope_text` builds it).

- [ ] **Step 1: Write the failing tests:**
  - `a_handoff_in_a_team_with_standing_qa_spawns_a_fresh_qa_at_its_commit` — team with `team_qa` and no QA member → `hand_off` → `pending_spawns` has one Qa spawn with `commit_id == COMMIT_A`, directory = developer's checkout from the handoff, brief containing `COMMIT_A` and the standing brief; no `qa_request` message queued.
  - `the_previous_qa_is_unlinked_and_marked_to_exit` — bind the first QA spawn, hand off `COMMIT_B` → QA member removed, `superseded_qa()` returns that session, a new Qa spawn exists for `COMMIT_B`.
  - `two_quick_handoffs_cancel_the_unstarted_older_qa_spawn` — two handoffs before any start → older spawn `cancelled`, one pending.
  - `earlier_findings_travel_in_the_next_qa_brief` — findings sent on the first handoff appear in the second QA spawn's brief.
  - `teams_without_standing_qa_keep_todays_qa_request` — `linked()` team (QA member, no `team_qa`) → unchanged behavior: one `qa_request` message, no spawn.
- [ ] **Step 2: Run** `cargo test --offline -p cockpit --test team_store` — fail.
- [ ] **Step 3: Implement** in `handoff`: when `team_qa` exists for the task, cancel pending (unstarted) Qa spawns, unlink a current QA member (cancel its messages; record it in a `qa_exits (session_id PRIMARY KEY, status)` table), and insert a Qa spawn with `brief = qa_brief_text(...)`; skip `schedule_qa`. `schedule_qa` stays for teams without `team_qa`.
- [ ] **Step 4: Run** — pass.
- [ ] **Step 5: Commit** `git commit -m "Each handoff starts a fresh QA at its commit when the team asked for QA"`

### Task 4: The daemon starts, binds and retires sessions

**Files:**
- Modify: `apps/desktop/src/registry.rs` (`System::start_detached`, `Entry::spawn_token`, `trusted_spawn_token`), `apps/desktop/src/teams.rs`
- Test: `apps/desktop/src/tests.rs`

**Interfaces:**
- Consumes: Tasks 1–3 store functions.
- Produces:
  - `System::start_detached(&mut self, command: &[&str]) -> bool` (spawn, never wait; default impl `false`; `Proc`-backed impl uses `Command::spawn` with stdio null)
  - `pub fn trusted_spawn_token(table: &dyn Processes, sender: &str) -> Option<String>` — `CLAUDE_SESSIONS_TEAM_SPAWN` from `first_process_outside`'s environment, only 32 lowercase hex; stored on the session `Entry` at its report (`spawn_token: String`, persisted in the snapshot like `runtime`)
  - `teams::start_spawns(registry: &mut Registry)` — `expire_spawns`, then for each `pending_spawns` row: `start_detached(["claude-sessions", "--agent", agent, "team-spawn", token, role, directory, commit_or_dash, brief])` and `mark_spawn_started`; for each `superseded_qa`: `start_detached(["claude-sessions", "exit", session])` and `mark_qa_exited`. Called after a successful host `Approve` and after an agent `Handoff`.
  - `live_sessions` binds: for each live session with a non-empty `spawn_token` not yet a member, `bind_spawn`.

- [ ] **Step 1: Write the failing tests** (extend `FakeSystem` with recorded `started: Vec<Vec<String>>`, `default_agent: Option<String>`, and per-peer spawn tokens):
  - `approving_a_recruit_request_starts_each_teammate_detached` — recruit from peer 20 (developer), approve → one recorded start `["claude-sessions","--agent","claude","team-spawn",<32hex>,"designer",<project dir>,"-",<brief>]`.
  - `a_session_launched_with_its_token_joins_the_team_as_its_role` — a new peer reporting with that token in its launch environment → after a `Context` request, it is the designer member.
  - `a_token_in_a_sessions_own_report_or_an_unknown_token_joins_nothing` — token only in the report JSON → not a member; unknown 32-hex token in launch env → not a member.
  - `a_handoff_exits_the_previous_qa_and_starts_the_next` — after binding a QA spawn and a second handoff → recorded `["claude-sessions","exit",<qa id>]` and a new `team-spawn … qa … <commit>` start.
- [ ] **Step 2: Run** `cargo test --offline -p desktop --lib` — fail.
- [ ] **Step 3: Implement**, replacing Task 2's empty directory map with live sessions' `cwd`.
- [ ] **Step 4: Run** `cargo test --offline -p desktop --lib` — pass.
- [ ] **Step 5: Commit** `git commit -m "Daemon starts approved teammates and binds them by launch token"`

### Task 5: `claude-sessions team-spawn`

**Files:**
- Modify: `config/bin/.local/bin/claude-sessions` (`team_spawn_session`, dispatcher), `config/bin/.local/bin/claude-sessions-agents/claude.sh` and `opencode.sh` (pass `CLAUDE_SESSIONS_TEAM_SPAWN` in background/detached launches)
- Test: `apps/cockpit/tests/claude_sessions_script.rs`

**Interfaces:**
- Consumes: the argv Task 4 sends.
- Produces: `claude-sessions [--agent A] team-spawn <token> <designer|developer|qa> <dir> <commit|-> <brief>`; exit 0 once launched; non-zero with a message on any invalid argument.

- [ ] **Step 1: Write the failing tests:**
  - `team_spawn_starts_a_designer_out_of_sight_in_the_askers_folder` — `team-spawn <hex32> designer <project> - "Design the jobs list"` → a background launch in `<project>` whose env contains `CLAUDE_SESSIONS_TEAM_SPAWN=<hex32>` and whose prompt starts with the brief label `Brief from your team (approved by Theo)`.
  - `team_spawn_starts_qa_in_a_fresh_copy_at_the_commit` — `team-spawn <hex32> qa <project> <40hex> "Review"` → a clone named `qa-<first 12 of commit>` is made and a `switch --detach <commit>` runs inside greywall; the launch's `-c` is the clone.
  - `team_spawn_refuses_bad_arguments` — token not 32 hex, unknown role, qa without a 40/64-hex commit, designer with a commit, missing folder, empty brief → non-zero, no tmux launch.
  - `opencode_team_spawns_carry_the_token_too` — `--agent opencode team-spawn …` → `tmux new-session` call contains `CLAUDE_SESSIONS_TEAM_SPAWN=<hex32>`.
- [ ] **Step 2: Run** `cargo test --offline -p cockpit --test claude_sessions_script team_spawn` — fail. (Needs the clone's release binaries; if missing, ask Theo to rebuild them with ask-to-run before this step.)
- [ ] **Step 3: Implement** `team_spawn_session` using `background_session` for designer/developer and `clone_for` + `git_in_sandbox switch --detach` + `agent_${launch_agent}_detached` for qa; set a global `launch_team_spawn` the agent launchers add to their `env` lists as `CLAUDE_SESSIONS_TEAM_SPAWN="$launch_team_spawn"` (empty otherwise). Prompt = `Brief from your team (written by the asking agent, approved by Theo):` + newline + brief.
- [ ] **Step 4: Run** the script suite — pass.
- [ ] **Step 5: Commit** `git commit -m "claude-sessions team-spawn starts an approved teammate with its token"`

### Task 6: Agents ask, Theo sees it

**Files:**
- Modify: `apps/cockpit/src/bin/cockpit-team.rs` (help example for `recruit`), `apps/cockpit/src/teams/view.rs` (recruit proposals, spawns), `apps/cockpit/src/app.rs` (nothing new to press: `a`/`x` already act on proposals)
- Modify: `config/claude/.claude/CLAUDE.md`, `config/opencode/.config/opencode/AGENTS.md` (when to ask), `README.md` (Linked sessions section)
- Test: `apps/cockpit/tests/team_ui.rs`, `apps/cockpit/tests/team_client.rs`

**Interfaces:**
- Consumes: snapshot fields `recruits`, `spawn_agent`, `spawns` (Tasks 1–2).

- [ ] **Step 1: Write the failing tests:**
  - `team_ui.rs`: `a_recruit_request_shows_the_asker_each_new_teammate_its_agent_and_brief` — detail contains `asker (developer): <id>`, `new designer (claude): Design the jobs list`, `QA: a fresh reviewer for each handoff (claude)`, and the brief text escaped (an `\u{1b}` in a brief shows as `\u001b`).
  - `team_ui.rs`: `a_failed_spawn_is_listed_and_counted` — row text contains `did not start` and the role.
  - `team_client.rs`: `help_shows_send_examples_that_parse_as_actions` keeps passing with a new `{"action":"recruit",…}` example line (it already parses every `{"action"` line).
  - `team_client.rs`: `recruit_is_an_agent_action` — `cockpit-team recruit` with a recruit JSON reaches the socket as `{"action":"recruit",…}`.
- [ ] **Step 2: Run** `cargo test --offline -p cockpit --test team_ui --test team_client` — fail.
- [ ] **Step 3: Implement** view rows/detail and help text; add the when-to-ask rules (3 lines, both instruction files): UI change without a FigCraft design → `cockpit-team recruit` a Designer before UI code; a Designer with an approved design and no Developer → recruit a Developer (brief: build from the approved frames); finished, tested work in a team → `cockpit-team handoff`. README: one bullet describing recruit requests, per-handoff QA and auto-exit.
- [ ] **Step 4: Run** the full workspace: `cd apps && cargo test --offline --no-fail-fast` — expect all pass.
- [ ] **Step 5: Commit** `git commit -m "Agents ask for teammates; the Team view shows recruits and failed starts"`
