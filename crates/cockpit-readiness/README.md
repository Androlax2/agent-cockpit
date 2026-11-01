# cockpit-readiness

Owns the readiness report: one status and a next action per requirement, from facts the
daemon gathers from their owners and the project's configuration. Design:
[report 09](../../docs/design/greenfield/09-project-setup-readiness.md). Every requirement that
is not available names the action that makes it so. It assesses facts; it decides nothing and
grants nothing.

**Depends on:** `cockpit-config` (the effective configuration, owner and project, with each
value's source), and `serde_json` to read a Docker engine's answer. The
`readiness -> config only` rule of `cockpit-workspace-rules/tests/crate_graph.rs` keeps every
other Cockpit crate out: the daemon gathers each fact from its owner (tmux, the sandbox engine,
the providers, resource admission, the workflow presets, the project registry) and passes it in.
**Used by:** `cockpit-daemon`, which serves the report as `readiness`; `cockpit doctor` renders
it (#50).

## Statuses

| Status | Meaning | Blocks |
| --- | --- | --- |
| `available` | Works now. | no |
| `needs-authorization` | Works once the owner grants it; the repository cannot grant itself. | no |
| `missing-prerequisite` | Something to install, start, fix or move first. | yes |
| `unsupported` | This host or platform cannot provide it. | yes |
| `unknown` | Cockpit cannot tell yet. | no |

`Requirement::needs` refuses a status other than `available` without a next action.
`Report::is_ready` is false while a requirement blocks; `cockpit doctor` then exits 1.

## Requirements

Host: `config`, `platform`, `tmux`, `sandbox`, `provider <name>` and its `sign-in`,
`resource controls`. With a project: `project`, `project trust`, `workflow preset` (its
steps' provider capabilities against the providers available), `execution environment`,
`needs` (against the host's budget), one `check <name>` each (its working directory), and
`docker` when the project or one of its checks runs in Docker.

Docker is probed only then, with `docker info` (10 s at most): a `docker` program is not an
engine. The engine's ID, name and version identify it, and a rootful engine on Linux is
`missing-prerequisite`: whoever reaches its socket is root on the host (report 11). The probe
moves to `cockpit-containers` when that crate exists (#390).

Provider sign-in is `unknown` until the adapters can report it (#443).

A provider the daemon wires but cannot use (not installed, or failing its version) is
`available` with `not usable, optional` and how to install it, as an unsandboxed owner is: a
host needs one usable provider, not all of them (#411). `providers` blocks when none is
usable, and a project's `workflow preset` blocks when no usable provider reports the
capabilities its steps need.
