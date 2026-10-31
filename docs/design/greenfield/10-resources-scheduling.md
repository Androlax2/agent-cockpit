# 10. Resources and scheduling

Status: proposed; defaults and cross-platform guarantees `validate`

## Decision

One shared capacity budget for everything Cockpit runs: agents, builds, tests, containers.
`cockpit-resource-admission` decides whether work fits; `cockpit-scheduling` decides what runs
where and explains waiting. Provider quotas are a separate constraint.

## Rules

- **Reserved, observed, enforced** are three separate values per execution. Report all three.
- **Verify enforcement.** Rootless Docker ignores `--cpus`/`--memory` without cgroup v2 +
  systemd, and only `memory`/`pids` controllers are delegated by default. Linux/WSL: inspect
  delegated controllers and effective limits; macOS: advertise only controls the selected
  backend verified. A required control that is unavailable holds that execution with a reason.
- CPU weight is contention priority; quota is a ceiling. Never present one as the other.
- **Fairness by project, then task.** Parallel checkouts and extra sessions share a project's
  allocation; fan-out never multiplies entitlement. Waiting work ages; a large feasible job
  must eventually accumulate enough capacity to start. Startup headroom for unfamiliar
  workloads; unstarted reservations expire; running allocations release only after observed
  termination.
- **Routine dispatch is deterministic**: capacity changes trigger scheduling without a Chief
  turn.
- **Queue explanations**: why waiting, limiting resource/backend, reservation, priority/aging,
  next re-evaluation, available action.
- **Pause has three distinct operations**: pause queue (start nothing new); pause at
  checkpoint (ask running work to stop at a supported boundary); stop execution (terminate the
  managed tree and confirm observed state). A paused process still holds memory and locks.
- Foreground/gaming profile lowers background admission and ceilings; agents cannot self-award
  foreground priority.
- Control-path capacity is separate from workload capacity (report 03).

## Rejected

- In-memory permits as the only record of capacity (lost on task drop; report 03).
- Generic GPU protection claims without a verified backend.

## Validate

- Three projects under foreground load: effective limits verified, headroom preserved, fair
  progress, session fan-out does not raise entitlement, infeasible work has an explainable blocker.
- Log flood + stalled viewer + stalled provider: memory and queues bounded; stop requests
  serviceable; cancelling a descendant-spawning workload ends the whole managed boundary.

## Sources

- Docker rootless resource limits: <https://docs.docker.com/engine/security/rootless/tips/#limiting-resources>
- systemd resource control (weight vs quota, effective limits):
  <https://www.freedesktop.org/software/systemd/man/latest/systemd.resource-control.html>
- Existing gaming design: `docs/superpowers/specs/2026-10-28-gaming-resource-protection-design.md`
