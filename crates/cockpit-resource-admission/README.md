# cockpit-resource-admission

Owns whether work fits the shared capacity budget: durable reservations, the limits
requested, supported and enforced per execution, the foreground profile and the three pause
operations.

One budget covers everything Cockpit runs: agent turns, deliveries, approvals, builds, tests
and containers (report 10). `cockpit-scheduling` decides what runs where and explains
waiting; this crate answers only whether a piece of work fits now, and what is in force once
it runs.

## Reservations

A reservation is a row in the state database, never an in-memory permit, so it outlives the
Rust task that asked for it and a daemon restart (report 03).

1. `admit` decides a request and, when it fits, commits its reservation in the caller's
   write transaction: the decision and the charge are one change. Held work gets a `Hold`
   that names its cause: the paused queue, a required control the host cannot enforce, a
   demand that can never fit, or one that does not fit yet.
2. `bind_execution` ties it to the `cockpit-processes` execution that started.
3. `sweep` releases an unstarted reservation past its expiry, and a running one only once
   `cockpit-processes` observed its execution ended. A dropped task releases nothing.
4. `hold` decides like `admit` but commits a `held` reservation for work no execution of
   its own carries, such as an agent turn `cockpit-scheduling` placed in a long-lived agent
   (#386). It never expires and no sweep releases it: its holder calls `release_held` once
   the work ended. Every decision and `in_use` count it like any other.

Reserved, observed and enforced are separate values: the reservation is what the budget is
charged; `Backend::inspect` reads what the execution uses and what the kernel enforces.

## Foreground profile

The owner's profile is `normal` or `foreground` (gaming). Under `foreground`, heavy local jobs
(builds, tests, containers) together get at most a share of the budget (25 % by default),
and each gets ceilings: the lowest CPU weight, a CPU quota no larger than its reservation,
`SCHED_IDLE`, nice 19 and the idle IO class. Agent turns, deliveries and approvals are never
held by the profile (decided 2026-10-31; the legacy cockpit's Option C stays legacy-only).
Only the owner's socket changes the profile: agents cannot award themselves foreground
priority.

What agents run inside their sessions is not a reservation, so it gets its own limits
(#1351, owner's decision of 2026-11-10), `Policy::session_limits`: nice 19 and the idle IO
class in every profile, so on an idle host a session loses nothing; under `foreground` also
the lowest CPU weight, `SCHED_IDLE` and a CPU quota of the foreground share of the budget. A
session is never held. `Backend::apply_to_session` sets them on every thread of a running
session's scope or process group, live and both ways; the daemon calls it on each profile
change and before a new session's launcher releases its agent. `Backend::inspect_session`
reads them back. A niceness is only ever raised. `SCHED_IDLE` is set on a running session
only where the daemon could take it off again (`RLIMIT_NICE` of at least 1); otherwise
`Backend::session_support` reports it unsupported and it is never set.

## Low-disk guard (#406)

The daemon's disk guard reads the free space of the state directory's filesystem every 30
seconds and records it with `Backend::set_disk_space`. While it is below
`storage.low_disk_floor_mib` (default 10240; 0 turns the guard off), `Policy::low_disk` is set
and `admit`, `hold` and `check` return `Hold::LowDisk` for agent turns and heavy jobs.
Deliveries and approvals keep going. The hold shows in `queue explain` and the TUI queue
(reason `low-disk`) and in `doctor` (requirement `disk`). Low space also wakes the
target-pruning pass at once, which deletes only what `[storage]` allows; the guard deletes
nothing itself. The owner is told through the daemon log, `doctor` and, once per entry into low
disk, `torlek-notify`, which the guard reaches through a `disk` entity on the change feed (#1143).

## Three pause operations

- **Pause queue** (`set_queue_paused`): nothing new is admitted; running work is untouched.
- **Pause at checkpoint** (`request_checkpoint`): records that a running reservation's work
  should stop at its next supported boundary; the work's owner reads it.
- **Stop execution**: the daemon terminates the execution through `cockpit-processes` and
  releases the reservation once its end was observed.

Freeze and thaw (`Backend::freeze`) stop processes where they are; a frozen execution still
holds its memory and locks, so it keeps its reservation.

## Backends

Nothing is claimed without evidence. `Backend::detect` probes the host once:

- **Linux and WSL 2**, executions in systemd user scopes: the controllers delegated to this
  user's manager (`cgroup.controllers` of the control group `systemctl --user show` names)
  decide `cpu-weight`, `cpu-quota` (`cpu`), `memory-max` (`memory`) and `tasks-max`
  (`pids`); `freeze` needs `cgroup.freeze`. Limits are applied with `systemctl --user
  set-property --runtime` once the scope runs, and read back from the scope's cgroup files.
- **Process groups** (macOS, or Linux without a reachable user manager): no cgroup limit;
  `freeze` is `SIGSTOP` to the group.
- **Shims**, wherever the program exists: `nice -n` and, on Linux, `chrt --idle 0` and
  `ionice -c 3` start the program, so every child inherits them. macOS has no `SCHED_IDLE`
  and no IO classes.

A control a request requires but the backend cannot enforce holds the work with the
backend's reason.

## Dependency edges

- Depends on: `cockpit-turso` (its tables), `cockpit-processes` (an execution's containment
  and its observed end), `rustix` (`SIGSTOP`/`SIGCONT` without `unsafe`), `tokio`.
- Never depends on scheduling, sessions or tasks: `cockpit-scheduling` calls it
  (`scheduling -> admission`), not the reverse. `tests/crate_graph.rs` enforces it.
- Used by: the daemon, which serves `torlek resources`, sweeps reservations and applies a
  profile change to running heavy executions.

## Tests

- `tests/admission.rs`: the budget, the foreground share, pause queue, required controls, and
  reservations that survive a reopen and release only on an observed end, on a real state
  database with real processes.
- `tests/held.rs`: held reservations charge every decision until their holder releases them.
- `tests/backend.rs`: the Linux backend through a fake `systemctl` and a fake cgroup tree,
  the shims, and freeze/thaw of a real process group.
- `tests/sessions.rs`: session limits on a real process group, set and taken off again,
  and `SCHED_IDLE` never set where it could not be undone.
- `tests/host.rs`: this host's own report, run on Linux, macOS and WSL 2 in CI.
- `tests/crate_graph.rs`: no Cockpit dependency but `cockpit-turso` and `cockpit-processes`.
