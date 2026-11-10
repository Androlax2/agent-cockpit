# Resource admission is durable reservations against one budget, enforced by what each host proves

Issue #42. Decided by the implementing session, within report 10's proposals and Theo's
2026-10-31 decision that the foreground profile never holds agent turns, deliveries or
approvals.

- **Reservations live in the state database, not in memory.** Each admitted job holds a
  `resource_reservations` row that charges the budget until it is withdrawn, its unstarted
  expiry (10 minutes) passes, or the daemon's sweep observes its execution ended. A
  reservation survives a daemon restart, and one whose execution ended while the daemon was
  down is released at start.
- **The foreground profile caps heavy work at 25 % of the budget and adds ceilings to it.**
  Builds, tests and containers get CPU weight 1, a CPU quota no larger than their demand,
  SCHED_IDLE and nice 19. Agent turns, deliveries and approvals are neither counted against
  the share nor given ceilings. On a profile change the daemon re-applies the cgroup ceilings
  to running heavy work; shims (nice, SCHED_IDLE) apply only from the next launch, because a
  running process tree cannot be re-wrapped.
- **The three pause operations.** Pause queue holds every new admission, whatever its
  workload, until it resumes; running work continues. Pause at checkpoint records the
  request on a running reservation for its owner to honor. Stop execution terminates the
  whole execution and releases its reservation only once the end is observed. Freeze and
  thaw are backend operations with no verb yet: their callers (the game guard's failsafe)
  come with scheduling.
- **Each host reports what it can enforce, with the reason when it cannot.** Linux reads the
  systemd user manager's delegated controllers and the scope's cgroup files; macOS has nice
  and SIGSTOP/SIGCONT and reports every cgroup control unsupported. WSL is probed like Linux
  rather than hard-coded to the macOS subset: a WSL 2 distribution with systemd gets its
  delegated controllers, and one without a user manager falls back to process groups, which
  is the nice and SIGSTOP subset the coverage map names. Work that requires a control the
  host cannot enforce is held with that reason.
- **Limits on a systemd scope are applied right after it starts.** cockpit-processes builds
  the `systemd-run --scope` call and takes no unit properties, and admission adds none to
  it, so a scope runs unenforced for the moment
  between its start and `systemctl set-property --runtime`. Shims are applied at launch.
- **Every resource command is owner-only.** An agent can neither award itself the
  foreground profile nor pause, check, checkpoint or stop other work.

Rejected:

- In-memory permits: lost on restart, so a restarted daemon would over-admit.
- Porting the legacy Option C (agent turns continue during games while the game guard holds
  heavy jobs): it stays legacy-only; the foreground profile replaces it.
- Pinning heavy work to cores (the legacy game guard's core pinning): not in this issue;
  left for a follow-up.

## Revisited 2026-11-10 (#1351)

Theo's decision: what agents run inside their sessions always yields, and yields harder under
`foreground`. Builds and tests an agent runs in its session are not heavy reservations, so
the 2026-10-31 rule left them unlimited while the owner played. Sessions are still never held.

- **Every session process runs at nice 19 in the idle IO class, in every profile.** On an
  idle host they lose nothing; they yield only when something else wants the CPU or disk.
  Torlek does not detect gaming or watch CPU load.
- **Under `foreground`, sessions also get the heavy jobs' hard ceilings:** `SCHED_IDLE`, CPU
  weight 1 and a CPU quota of the foreground share of the budget (25 % by default). The
  daemon walks the session's scope or process group and sets each thread live, so neither
  switch needs a relaunch; a new session gets its profile's limits before its launcher
  releases the agent.
- **`SCHED_IDLE` is never set where the daemon could not take it off.** An unprivileged
  process leaves SCHED_IDLE only within `RLIMIT_NICE` (at least 1 for nice 19). Without it
  `doctor` and `resources status` name it as never set on sessions here, rather than leaving
  a session slowed after the game ends. Nice 19 is never undone, so it needs no such limit.
- **Rejected:** the daemon watching CPU pressure and switching the profile itself (it cannot
  tell the owner's game from its own builds, and it flaps); a launcher hook alone (running
  sessions would keep their priority); re-launching sessions in shims (loses the agent's
  turn and terminal).
