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

Reserved, observed and enforced are separate values: the reservation is what the budget is
charged; `Backend::inspect` reads what the execution uses and what the kernel enforces.

## Foreground profile

The owner's profile is `normal` or `foreground` (gaming). Under `foreground`, heavy local jobs
(builds, tests, containers) together get at most a share of the budget (25 % by default),
and each gets ceilings: the lowest CPU weight, a CPU quota no larger than its reservation,
`SCHED_IDLE` and nice 19. Agent turns, deliveries and approvals are never held or limited by
the profile (decided 2026-10-31; the legacy cockpit's Option C stays legacy-only). Only the
owner's socket changes the profile: agents cannot award themselves foreground priority.

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
- **Shims**, wherever the program exists: `nice -n` and, on Linux, `chrt --idle 0` start the
  program, so every child inherits them. macOS has no `SCHED_IDLE`.

A control a request requires but the backend cannot enforce holds the work with the
backend's reason.

## Dependency edges

- Depends on: `cockpit-turso` (its tables), `cockpit-processes` (an execution's containment
  and its observed end), `rustix` (`SIGSTOP`/`SIGCONT` without `unsafe`), `tokio`.
- Never depends on scheduling, sessions or tasks: `cockpit-scheduling` calls it
  (`scheduling -> admission`), not the reverse. `tests/crate_graph.rs` enforces it.
- Used by: the daemon, which serves `cockpit resources`, sweeps reservations and applies a
  profile change to running heavy executions.

## Tests

- `tests/admission.rs`: the budget, the foreground share, pause queue, required controls, and
  reservations that survive a reopen and release only on an observed end, on a real state
  database with real processes.
- `tests/backend.rs`: the Linux backend through a fake `systemctl` and a fake cgroup tree,
  the shims, and freeze/thaw of a real process group.
- `tests/host.rs`: this host's own report, run on Linux, macOS and WSL 2 in CI.
- `tests/crate_graph.rs`: no Cockpit dependency but `cockpit-turso` and `cockpit-processes`.
