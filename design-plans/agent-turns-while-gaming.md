# Option C: let agent turns continue during games

Status: **implemented** (Theo approved on 2026-10-31: guard-verified `gaming_guarded` for Chief messages, answers, corrections, form replies and the Chief wake; setting `chief-turns-while-gaming`, default on). Sections below are the original proposal; README describes what exists.
Origin: todo 0990586d. Chief messages to two gaming-blocked sessions expired after 10 minutes and were
cancelled (`6afdfdcb…`, `2ea3bf1b…`). Option B (in this branch) only records why a message was cancelled. It is
diagnostics, not during-game continuation.

## 1. The distinction this proposal rests on

Two different things are called "gaming protection" today:

| | Agent-turn admission | Heavy local-job protection |
| --- | --- | --- |
| What it guards | Whether an idle agent session is handed a prompt (Chief message, todo gift, answer, wake, queue start) | CPU, disk, RAM, GPU used by builds, tests and containers on this machine |
| Cost of one admission | One LLM turn: network I/O and a node/bun process mostly waiting on the API. Light locally. | Whatever the turn *runs*: `cargo`, `make`, `pnpm`, `pytest`, Docker builds. Can be heavy. |
| Gaming signal | Focus-based: `gamemode` submap or the *focused* window is a game (`apps/cockpit/src/util.rs:355`, `apps/osd/src/registry.rs` `gaming()`, 2 s cache). Alt-tab ends it. | Lifetime-based: any game window open, plus 30 s (`config/bin/.local/bin/cockpit-game-guard`) |
| Today | Blocks almost everything (table §2) | Only CPU affinity pinning of `SCHED_IDLE` build threads to 2 cores (commit 1ca1228). Service not enabled by the install. |

Option C relaxes **admission** for a bounded set of deliveries. It keeps **job protection** as it is or makes
it stronger. It relies on job protection being in force: an admitted turn may start a build, and that build
must be the thing that gets limited.

## 2. Exact gates involved

Admission gates that currently refuse during games (line numbers on `origin/main` 1ca1228):

| # | File:line | Gate | Proposed under C |
| --- | --- | --- | --- |
| G1 | `apps/osd/src/chief.rs:137`, `:141` (`workers_among`) | Pushes `"gaming"` into `blocked_by` (focus report and the daemon's own check). It can appear twice. | Keep pushing it. The reason stays visible to the Chief. |
| G2 | `apps/cockpit/src/chief/store.rs:1539` (`claim`, `chief_message`) | Tolerated blockers: `live_queued_admission_verification_required`, `legacy_work_in_progress` | Also tolerate `gaming`, **only when the job guard is confirmed active** (§3). |
| G3 | `store.rs:1631`, `:1634` (`ANSWER_IGNORES`, `FORM_REPLY_IGNORES`) | Theo's answers and corrections and the Chief's answers wait during games | Also tolerate `gaming` (Theo's own answers are the strongest case), behind the same guard check. |
| G4 | `store.rs:1160` (`claim_todo_gift`) and `apps/osd/src/chief.rs:47`, `:50` (`legacy_auto_ready`) | New todo work does not start during games | **Unchanged.** A new task is the most likely start of a heavy job. |
| G5 | `apps/cockpit/src/queue.rs:56`, `apps/cockpit/src/app.rs:1464` | Queue dispatch and fresh-session starts pause | **Unchanged.** |
| G6 | `store.rs:1320` (`wake_ready`, `chief_wake`) | The Chief itself is not woken during games | Option: tolerate `gaming` so the Chief can coordinate. It is a pure coordination turn. Separate approval item. |
| G7 | `SESSION_START_OPEN_MS` (`apps/cockpit/src/chief.rs:141`), used as the Chief-message expiry (`store.rs` `request_message`) | A message held longer than 10 minutes is cancelled | Unchanged under C, since messages would no longer be held for gaming. Alternative A (pause the clock while gaming) is not proposed. |

Gates that stay untouched in every variant:

- admission verification (`live_queued_admission_verification_required` is already tolerated and stays as it is);
- the active Chief grant and epoch;
- the exact recipient identity and runtime (a restart lapses the message);
- `native_busy`, `native_inbox_or_input_pending`, `human_input` / `needs-you`, `viewed_session`, `idle_guard`;
- `restart_pending`, `host_command_or_approval_pending`, `host_sleep_in_progress`, `human_prompt_queued`;
- the legacy launch reservation (`launching`);
- the 20 messages an hour cap and one waiting message per session;
- the uncertain-delivery tombstones: no resend and no replay.

Gates not in the table never blocked for gaming and are not affected: team and link messages
(`apps/cockpit/src/teams/store.rs` `claim`), `claim_check`, `ReserveResume`, and subscription-fallback native
prompts.

## 3. The job guard condition (what makes C safe)

C admits a turn during a game only if all of these hold. Each is checked by the daemon, never reported by an agent:

1. `cockpit-game-guard` is running and protecting. A readable `$XDG_RUNTIME_DIR/cockpit-game-guard.json`
   with `"protecting": true` and a fresh `updated_at` (under 10 s old; how often the guard rewrites the file
   while protecting must be checked before implementing). Without it, C behaves exactly like today.
2. The recipient is an OpenCode or Claude session started by the greywall launcher, so its build tools go
   through the `SCHED_IDLE` shims (`config/bin/.local/bin/agent-build`).
3. Not in the `gamemode` submap. The submap is Theo's explicit "freeze everything" and keeps today's behaviour.

A new blocked reason, `gaming_job_guard_inactive`, would replace `gaming` when condition 1 fails, so the Chief
can see why.

## 4. Limitations under C (stated plainly)

- **CPU:** pinning only. No CPU-time quota (`CPUQuota`) and no cpuset cgroup. Two cores at full load still
  share L3 cache and memory bandwidth with the game (the 7800X3D's V-cache is shared). New builds start and
  are pinned within about 2 s, so there is a short burst on any core.
- **Disk I/O:** none. The `io` controller is not delegated and the NVMe uses the `none` scheduler. A large
  `cargo` build or `docker build` can cause game asset-streaming stutter.
- **RAM:** none (`MemoryHigh` / `MemoryMax` are not set). A link step or a test suite can push the game into
  swap.
- **Docker:** not covered. `agent-docker@` runs as another uid in `system.slice`, outside the guard's reach.
  A turn that runs `docker build` or `compose up` is unprotected.
- **Untracked work:** commands called by absolute path, or not started through the shims, are not pinned
  (the guard status only lists them as "untracked").
- **GPU and network:** not limited. LLM API traffic is small. Model downloads or `pnpm install` are not.
- **Signal mismatch:** admission uses the focus-based signal and the guard uses the lifetime-based one. After
  alt-tab, admission considers the game over while the guard still protects (safe in this direction). The
  reverse cannot happen.

Because of the Docker, RAM and I/O gaps, C is only as safe as Theo's tolerance for those. A stricter variant,
C-strict, would also hold admission for sessions whose project has an agent Docker daemon
(`~/agent-cockpit` `agent-docker-add`) until a Docker limit exists.

## 5. Implementation sketch (for the later approval)

1. `apps/osd/src/chief.rs` `workers_among`: read the guard state (new small reader, fail closed). Push
   `gaming` when the guard protects, otherwise `gaming_job_guard_inactive`.
2. `apps/cockpit/src/chief/store.rs`: one named constant, `GAMING_ADMISSION_TOLERATED = ["gaming"]`, added to
   the tolerated lists of G2 and G3 (and G6 only if approved). The single named opt-out is used at every
   call site.
3. A setting, `chief-turns-while-gaming` (default **off**), in `apps/cockpit/src/settings.rs`, read where the
   daemon builds `blocked_by`. Off means `gaming` is never tolerated, exactly as today.
4. README: one line under the game-guard paragraph.

## 6. Tests to add (all must fail before the change)

- `chief_message_is_delivered_while_gaming_when_the_job_guard_protects_and_the_setting_is_on` (osd `chief_tests.rs`)
- `chief_message_still_waits_while_gaming_when_the_job_guard_is_missing_stale_or_off`, parametrised over a
  missing, a stale and a non-protecting state file
- `gamemode_submap_still_holds_every_chief_delivery`
- `todo_gifts_queue_dispatch_and_fresh_sessions_still_pause_while_gaming` (regression for G4 and G5)
- `gaming_tolerance_never_bypasses_busy_input_viewed_restart_launch_or_admission`: the existing
  `chief_messages_do_not_bypass_idle_input_focus_restart_or_gaming_checks`, split so `gaming` moves to the
  allowed case only with guard and setting
- `theos_answer_reaches_an_idle_session_during_a_game_with_the_guard` (`apps/cockpit/tests/chief_events.rs`)
- `the_setting_defaults_off_and_explains_itself` (`apps/cockpit/src/tests.rs`)
- Python: `tools/tests/test_cockpit_game_guard.py`, a state file freshness contract test

## 7. Rollback

- Immediate, no deploy: turn `chief-turns-while-gaming` off (`cockpit settings`). The next observation pushes
  `gaming` as a blocker again, and queued messages wait (and expire with the recorded reason from Option B).
- Full: revert the C commit. No schema change, no data migration, nothing to clean up. Messages already
  delivered stay delivered. Nothing is resent.
- Stopping `cockpit-game-guard.service` also reverts C in practice, because the guard condition fails closed.

## 8. Open questions for Theo

1. Should G3 (your own answers and corrections) be allowed during games even without the job guard? Answers
   resume tasks you already started.
2. Should the Chief itself wake during games (G6)?
3. C or C-strict (hold sessions with an agent Docker daemon)?
