# Gaming resource protection: design and plan (for Theo's approval)

Todo: d021dc46. Session: ses_e7cf8702effe7FJjkzvNkwfeS5. Status: **design only, nothing implemented,
no live resource change made.** Grounded in the running copy `~/agent-cockpit` at `f724daf`.

## 0. Step 1 (smallest safe deploy, requested 2026-10-28)

Scope: one Python script `config/bin/.local/bin/cockpit-game-guard` and one user unit
`cockpit-game-guard.service`. No Rust, no osd, no launcher, no shim change, no root.

- Every 5 s: `hyprctl clients -j`; a game is any open window matching `GAME_CLASS` (focused or
  not, so alt-tab keeps it) or the `gamemode` submap. Over after 30 s with none.
- While a game is open, every 2 s: processes with our uid, `SCHED_IDLE` (set by `agent-build`,
  inherited by children) and in an `app.slice/tmux-spawn-*.scope` cgroup are pinned to CPUs
  6,7,14,15 (2 cores). Original affinity recorded with pid+starttime in
  `$XDG_RUNTIME_DIR/cockpit-game-guard.json`.
- Game over, service stop, or service start with leftovers: original affinity restored.
- Separately approved: `cockpit setting freeze-while-gaming off` (stops the dotfiles freeze).
Deferred: queueing new jobs, memory gate, disk cap, Docker. Sections 2-6 remain the later steps.

## 0b. Step 2: heavy-job admission (implemented on feat/game-guard-admission, not deployed)

Ownership agreed: workflow owner ses_e7d89f02 (lock pair, base fc6df5d) and refactor owner
ses_e7e1238d (agent-build pair, base 4ea478a2 formatting); both reserve no part.

- Status `~/.cache/agent-shims/heavy-gate.json`, written only by the guard's detection loop
  (atomic, 0644; sandboxes see ~/.cache read-only). Fields: version 1, state (clear | gaming |
  memory | held | off), reason, since, updated_at, sequence, boot_id, writer, guard_pid, memory,
  manual_release_until. Rewritten every detection (5 s idle, each pin scan while gaming). A stop or
  `restore` never writes it, so a dead guard's status goes stale.
- Readers (`agent-build`; `full-test-lock.py` via `agent-build --wait-gate`, before `open_lock`)
  accept it only if: opened O_NOFOLLOW, regular, owned by them, <= 4 KiB, valid JSON and schema,
  finite times. Then: `off` runs (warning, any boot); a same-boot `manual_release_until` within
  24 h runs; otherwise it must be this boot and -5..30 s old, and state clear.
- Policy C (Theo, 2026-10-28): anything else holds until Theo acts on the host:
  `cockpit-game-guard release [MINUTES]` (1-1440, default 60; works with no guard; never refreshes
  updated_at), `hold` (also turns admission back on), `off`, `on`. Kept in
  ~/.local/state/cockpit/game-guard-manual.json, so they survive a reboot.
- Nested work (workflow review P1): admission is tracked by `AGENT_BUILD_ADMITTED=<pid>:<start time>`
  of the admitted process (agent-build execs into the tool, keeping its pid;
  full-test-lock.py marks its command with its own). It counts only while that exact process is an
  ancestor of the caller; priority alone never counts. `full-test-lock.py` no longer changes the
  suite's priority. Cheap calls are whole invocations only (`--version`, `-V`, `--help`, `-h` as
  the sole argument, `make help`, `cargo fmt ...`, `cargo metadata ...`); `make help test` waits,
  and a cheap call admits nothing below it.
- Memory hold: MemAvailable < 3 GiB or PSI some avg10 >= 5 %; release after 60 s at >= 5 GiB and
  PSI some avg60 < 1 %; unreadable memory holds. Every transition logged with its sample.
- After a hold: one job per 10 s through a flock ticket in ~/.cache/cockpit/test-lock, then the
  gate is checked again (a game may have started meanwhile).
- Waiting jobs print why once a minute, that nothing has run yet, and the release command.
- Deployment order: apply, then restart the guard at once (until the new guard writes the file,
  policy C holds every new heavy job, including a host `make build` in after-apply when apps/
  changed). Existing sessions: ~/.cache is bind-mounted read-only, so the file appears live; their
  shims copy refreshes at the next session start.

### Step 2 review answers (Theo's questions, 2026-10-29)

- Stop policy (Theo chose B): a clean stop, restore, crash or restart never publishes, never marks
  admission off; the status goes stale and holds until `cockpit-game-guard release`/`off`.
- Admission proof: scheduling priority never counts (tests: SCHED_IDLE, SCHED_BATCH, nice 19 all
  wait). The ancestor marker is still cooperative: a session can deliberately forge it (it can
  already skip the shims with absolute paths). Long-lived admitted watchers (e.g. a dev server
  started through npm) start their rebuilds without waiting, CPU-pinned only.
- Existing sessions: the status file is live for them (~/.cache bind-mounted read-only). Their
  shims run ~/.cache/agent-shims/bin/agent-build, shared by all sessions and refreshed at every
  session start; until then they run the old runner (no gate). Deployment refreshes that shared
  copy once, the same atomic copy the launcher makes, so no session restart is needed.
- Direct runner: agent-cockpit's Makefile runs each checkout's own agent-build and
  full-test-lock.py. An old checkout's runner has no gate itself; inside a sandbox it finds the
  shared cargo shim first on PATH, and that shim now gates it (an old runner sets SCHED_IDLE but no
  marker, which the refactor review's reproduction relied on). On the host (no shims on PATH) an
  old checkout stays ungated until rebased onto Step 2.
- A run queued behind another clone's full suite checks admission again once it has the lock;
  if jobs became held meanwhile it gives the lock back and waits again (refactor review P1).
- Ordering: apply (after-apply builds only when apps/ changed; Step 2 changes no apps/ file, so no
  build) -> restart the guard -> refresh the shared shim copy -> verify. Between apply and the new
  guard's first detection (< 5 s) a new heavy job holds (C). Restart unpins pinned build threads
  for up to one scan (ExecStopPost restore) until the new guard pins them again.
- Manual release across reboot: the release time is absolute, saved in
  ~/.local/state/cockpit/game-guard-manual.json. Readers honour it only with this boot's id, so
  after a reboot it counts again once the guard has republished it, until it expires; with no
  guard running after a reboot it does not count (run release again). hold and off are saved
  there too and survive reboots; off is accepted from any boot until `on`.

## 1. What exists today (evidence)

| Piece | Where | What it does | Problem for this task |
| --- | --- | --- | --- |
| `freeze-while-gaming` setting | `settings.json`: **on** | dotfiles' `game-performance` freezes every sandbox scope (`cgroup.freeze`) during a game, lists them in `$XDG_RUNTIME_DIR/game-performance.frozen`, thaws on `off` | Freezes chat and edits too, and freezes processes holding locks (test lock, todo lock, approvals). Directly contradicts the approved scope. |
| `agent-build` shims | `config/bin/.local/bin/agent-build` | cargo, make, npm, pnpm, go, composer, pytest run at nice 19 + `SCHED_IDLE`, every CPU | Good base: every heavy job is already marked (`SCHED_IDLE` is inherited by children). No admission gate, no cap during a game. |
| Full-test lock | `tools/full-test-lock.py`, `make test` | one full run at a time, flock in `~/.cache/cockpit/test-lock` | A gate placed inside `agent-build` would wait *while holding* this lock. Lock ordering must be handled. |
| Failsafe | `apps/cockpit/src/failsafe.rs`, `cockpit-failsafe.timer` | freezes sessions when osd/sidebar/greyproxy runs away | Separate concern; must not fight it (never thaw what it froze, never let it freeze our state away). |
| Gaming gates | `osd/src/registry.rs` `gaming()`, `cockpit/src/util.rs gaming()`, `queue.rs`, `chief.rs` | **focus-based** (gamemode submap or *focused* game window): hold banners, approvals, queue dispatch | Coordination, not resources. Focus ends on alt-tab, so it cannot drive resource protection. Left unchanged. |
| Agent Docker | `system/etc/systemd/system/agent-docker@.service` | rootless dockerd per project, system service, user `agent-<name>`, `system.slice` | Other uid: the user cannot renice/pin its processes. Only root-level unit properties reach it. |

Host facts (read-only, today): Ryzen 7 7800X3D, 8 cores / 16 threads (siblings n and n+8), one
CCD with shared V-cache, NVMe with I/O scheduler `none`, zram swap 15.5 GiB, 30 GiB RAM.
`user@1000.service` delegates **cpu memory pids only** (no `io`, no `cpuset`). Research 96acba81:
builds 180-580 % CPU, agents ~0.4 core, Docker ~0 at the time.

Consequences:

- `SCHED_IDLE` already yields CPU time to the game, but builds still share L3 V-cache, SMT
  siblings, memory bandwidth and boost headroom. A *bounded CPU set* is what protects frame time.
- `ioprio` idle class is ignored by the `none` scheduler; `io.weight` needs BFQ or io.cost. Only
  `io.max` (absolute bandwidth/IOPS) works, and only where the io controller is delegated.

## 2. Behaviour (approved scope, made precise)

1. **Game lifetime, not focus.** Protected from the first game window (any Hyprland client whose
   class matches the existing `GAME_CLASS` list, or the `gamemode` submap) until no game window
   exists for 30 s. Alt-tab to the cockpit keeps protection.
2. **Chat, cloud reasoning, edits, Chief coordination continue.** Only processes already marked
   heavy (`SCHED_IDLE`, i.e. started through `agent-build`) are touched. Agent processes, the
   Chief, osd, sidebar, approvals and ask-to-run are never touched.
3. **New heavy jobs wait.** `agent-build` (and `full-test-lock.py` *before* taking the lock)
   prints `waiting: a game is running (since 21:04); starts when it closes` and polls the gate.
   Nothing is cancelled; the command starts unchanged later and its exit code is its own.
4. **Running heavy jobs are throttled, not stopped.** Every heavy process is pinned to one shared
   CPU set (aggregate cap whatever the number of jobs). No SIGSTOP, no freeze, no kill.
5. **Memory pressure blocks new heavy work** (gaming or not); it never kills or caps memory.
6. **Restore after the game.** Original CPU sets restored; waiting jobs released one at a time
   (staggered) so they do not stampede.
7. **Docker** (project rootless daemons only, never Theo's main Docker): same cap during a game
   through a root-owned helper (see Decision 3).

## 3. Design

### 3.1 `cockpit-game-guard` (new host user service)

New binary `apps/cockpit/src/bin/cockpit-game-guard.rs` + pure logic in
`apps/cockpit/src/game_guard.rs`, unit `config/systemd/.config/systemd/user/cockpit-game-guard.service`.
Outside the sandbox, outside osd (a busy osd must not delay protection; keeps osd/registry.rs
untouched, which the refactor owner is in).

- **Detection:** listens on Hyprland's event socket (`openwindow`/`closewindow`/`submap`) and
  reconciles with `hyprctl clients -j` every 15 s. Event-driven: near-zero idle CPU (budget test).
- **Gate file:** atomically writes `~/.cache/agent-shims/heavy-gate.json`
  (`{"state":"clear|gaming|memory","since":…,"updated_at":…,"reason":"…"}`). That folder is
  already readable and not writable in every sandbox, so no launcher change and no spoofing.
  Heartbeat every 15 s.
- **Throttle:** every 5 s while gaming, scans tmux-spawn scopes of agent sessions, finds
  `SCHED_IDLE` processes (field 41 of `/proc/<pid>/stat`), records `(pid, starttime, original
  affinity)` once, then `sched_setaffinity` to the game CPU set. Children forked later inherit it.
  Never touches the Chief's scope, non-idle processes, or other uids.
- **State:** `$XDG_RUNTIME_DIR/cockpit-game-guard.json` (not visible to sandboxes). On start with
  no game, it restores anything left over (crash safety). `starttime` guards pid reuse.
- **Restore:** game gone 30 s → restore recorded affinity; processes born during the game get
  their session's original set; gate → `clear`; waiting jobs see a `release_slot` sequence so one
  starts every 10 s.
- **Memory:** host computes from `/proc/meminfo` + `/proc/pressure/memory`; gate `memory` when
  over the threshold, `clear` after 60 s back under (hysteresis).
- **Failsafe coexistence:** affinity changes work on frozen processes too; the guard never
  freezes or thaws. Added to `failsafe::WATCHED` with its own CPU/RSS limit.

### 3.2 Admission: `agent-build` and `full-test-lock.py`

- The gate lives in `agent-build` (already copied into the sandbox shims, no new file to ship);
  `full-test-lock.py` calls `agent-build --wait-gate` before `flock`.
- An admitted job sets `AGENT_BUILD_ADMITTED=1`; nested `agent-build` calls (make → cargo) skip
  the gate. This is what keeps a running `make test` from blocking itself mid-run while holding
  the test lock: it is running work, so it is throttled, not queued.
- **Gate file missing, unreadable or stale (> 60 s):** the job runs, with a loud warning
  (`game guard not running: not protected`). Rationale: a dead guard must not hold all work
  forever; the warning and `cockpit doctor` make it visible.
- Exemptions (cheap, never wait): `cargo fmt`, `cargo metadata`, `--version`, `make help`.

### 3.3 Retire the freeze

`freeze-while-gaming` becomes "protect while gaming" (setting name unchanged, so no migration):
on → the guard is active; off → guard publishes `clear` and throttles nothing. The **freeze code
lives in dotfiles** (`game-performance`): it must stop freezing sandbox scopes. That is a
cross-project todo to the dotfiles session, after your approval. `thaw_frozen_pane` and the
`game-performance.frozen` reads stay until that change has been live for a while (old frozen
lists must still thaw).

### 3.4 Visibility

- Column card/banner text: reuses the existing "paused" counter in `app.rs:2069`
  (`game_frozen`), switched to read the guard state: `game: N jobs throttled, M waiting`. Wording
  only, no new UI element; if you want more than that, a Designer goes first.
- `cockpit game-guard status|restore` subcommand; `cockpit doctor`: state left with no game →
  fail with `cockpit game-guard restore`.

## 4. Decisions only you can make (I will ask one at a time)

1. **Retire the scope freeze** in dotfiles `game-performance` (it is on today and freezes chat).
   Recommended: yes.
2. **CPU cap during a game:** A) 4 threads = 2 cores, CPUs 6,7,14,15 (recommended: builds still
   progress, game keeps 6 cores); B) 2 threads (6,14); C) 6 threads.
3. **Docker during a game:** A) small root-owned helper `agent-docker-game@.service` + `.path`
   in `system/` that applies only `AllowedCPUs=` (same set) and `IOReadBandwidthMax`/
   `IOWriteBandwidthMax` with `--runtime` to `agent-docker@*.service` and clears them after
   (recommended; `make system` shows the diff and asks for sudo); B) static low `CPUWeight=10`
   only, always on, no disk cap; C) leave Docker unprotected for now.
4. **Disk cap for agent sessions:** needs `io` delegated to `user@1000.service` (drop-in
   `Delegate=cpu cpuset io memory pids`, host change, effective after re-login). A) do it, then
   the guard sets `io.max` on heavy session scopes during a game; B) skip: disk is protected only
   by queueing new jobs and the CPU cap (recommended for v1; revisit with measurements).
5. **Memory threshold:** hold new heavy work when MemAvailable < 4 GiB or memory PSI
   `some avg10` > 10 %; release at > 6 GiB and < 2 % for 60 s. Recommended as start values.

## 5. Not doing (explicitly)

No cancellation, replay, forced restart, SIGSTOP, freeze, memory kill or `MemoryMax`; no change
to approvals, ask-to-run, the protected Chief grant, admission/budget, Docker isolation; no
promise of zero stutter; no game or hardware test until you say you are ready.

## 6. Implementation plan (after approval)

Each step test-first, `make test` in the clone, real entry points.

1. `game_guard.rs` pure logic: game-lifetime state machine with 30 s hysteresis, memory gate,
   throttle/restore bookkeeping with pid+starttime. Parametrised unit tests (alt-tab keeps
   protection, close → restore after 30 s, pid reuse, crash-restore on start). ~2 h
2. `agent-build --wait-gate`, `AGENT_BUILD_ADMITTED`, stale/missing gate, exemptions;
   `full-test-lock.py` gates before `flock`. Python tests in `tools/tests/`
   (`test_agent_build.py`, `test_full_test_lock.py`): regression test that a waiting job never
   holds the test lock. ~1.5 h
3. `cockpit-game-guard` binary: Hyprland socket, /proc scan, `sched_setaffinity`, gate writer,
   unit file. Fixture tests with a fake `/proc`, fake hyprctl; one real test that pins and
   restores a child `SCHED_IDLE` process. Budget test: idle loop does ≤ K scans/min. ~3 h
4. Setting rewire, `cockpit game-guard status|restore`, doctor check, column wording, README. ~1.5 h
5. Docker helper (only if Decision 3A): unit + path files in `system/`, tests of the generated
   commands. ~1.5 h
6. Review, then `cockpit apply` via `ask-to-run`; dotfiles todo for `game-performance`.
7. Live verification only when you say you are ready: start a build, open a game, check affinity
   and gate, alt-tab, close, check restore. ~20 min of your time.

## 7. Shared files and owners

| File | Change | Possible overlap |
| --- | --- | --- |
| `config/bin/.local/bin/agent-build` | gate, `--wait-gate` | workflow owner (ses_e7d89f…) if touching build tooling |
| `tools/full-test-lock.py`, `tools/tests/test_full_test_lock.py`, `test_agent_build.py` | gate before lock | same |
| `apps/cockpit/src/lib.rs` | one `pub mod game_guard;` line | refactor owner (ses_e7e1238…, 5779dac9) |
| `apps/cockpit/src/failsafe.rs` | one `WATCHED` entry | refactor owner |
| `apps/cockpit/src/app.rs` | read guard state for the existing counter | refactor owner |
| `config/bin/.local/bin/cockpit` | subcommand + doctor check | refactor owner |
| `README.md` | one section | both |
| New files only: `game_guard.rs`, `bin/cockpit-game-guard.rs`, unit file, `system/` helper | — | none |

Not touched: `osd/src/registry.rs`, `osd/src/chief.rs`, `queue.rs`, launcher, greywall config.
I will ask both owners (link request, needs your approval) which of the shared files they have
uncommitted changes in, and rebase on theirs rather than overwrite.
