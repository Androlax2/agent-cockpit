# 09. Project setup and readiness

Status: proposed; clean-home acceptance `validate`

## Decision

Cockpit prepares a project deterministically and reports readiness before any agent starts.
Help, config inspection and diagnostics work even when tmux, a provider or the daemon is
unavailable, and always leave a next action.

## Project configuration (small, optional)

- Workflow preset and required provider capabilities.
- Expected execution environment (native, sandbox profile, Docker engine).
- Existing check commands with working directories; Cargo/Make/the project's runner stays the
  source of truth, Cockpit invokes and records.
- Resource needs and expected result artifacts.

**Repository configuration describes requirements; owner settings grant permissions.**
A repository cannot grant itself host access by editing its config. Project-level skills and
guidance load only for trusted projects (owner decision).

## Readiness report

Each requirement gets one status: `available`, `needs-authorization`, `missing-prerequisite`,
`unsupported`, `unknown`. Checked: selected provider and authentication, sandbox backend
and effective profile, tmux version, Docker engine identity and isolation (a `docker` binary
is not an engine), WSL distribution and filesystem location on Windows, enforced resource
controllers (report 10). Each non-available item names the next action.

## Verification receipts

A receipt identifies exact source (base/head, dirty state), command, execution environment,
outcome, bounded output and references to larger artifacts. Reuse requires matching inputs;
a changed artifact or check definition invalidates affected receipts. Receipts answer
"what was checked?" for the next agent and the human.

## Acceptance (validate)

- Clean home, two OS users, no Docker: separate state and sockets; restricted workloads fail
  with a precise reason; permitted host work still runs; restart recreates endpoints without
  losing durable state.
- Same with Docker present: engine identity verified, isolation reported honestly.
- A new user configures a project, launches a supported agent, retrieves the correct
  instructions and completes a permitted check with an understandable result.

## Sources

- XDG base directories: <https://specifications.freedesktop.org/basedir/latest/>
- Agent Skills trust/collision guidance: <https://agentskills.io/client-implementation/adding-skills-support>
