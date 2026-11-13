# cockpit-readiness

Owns the readiness report: one status and a next action per requirement, from facts the
daemon gathers from their owners and the project's configuration. Design:
[report 09](../../docs/design/greenfield/09-project-setup-readiness.md). Every requirement that
is not available names the action that makes it so. It assesses facts; it decides nothing and
grants nothing.

**Depends on:** `cockpit-config` (the effective configuration, owner and project, with each
value's source). The
`readiness -> config only` rule of `cockpit-workspace-rules/tests/crate_graph.rs` keeps every
other Cockpit crate out: the daemon gathers each fact from its owner (tmux, the sandbox setting,
the providers, resource admission, the workflow presets, the project registry) and passes it in.
**Used by:** `cockpit-daemon`, which serves the report as `readiness`; `torlek doctor` renders
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
`Report::is_ready` is false while a requirement blocks; `torlek doctor` then exits 1.

## Requirements

Host: `config`, `platform`, `tmux`, `sandbox`, `provider <name>` and its `sign-in`,
`resource controls`. With a project: `project`, `project trust`, `workflow preset` (its
steps' provider capabilities against the providers available), `execution environment`,
`needs` (against the host's budget), one `check <name>` each (its working directory),
`docker` when the project or one of its checks runs in Docker, and the launch gates of a
registered checkout.

## Launch gates

`launch_gates` turns what each gate of one launch found (#1250) into one `launch <gate>`
requirement each, in launch order: `command`, `backend`, `desktop session`, `sign-in`,
`github`, `toolchains`, `folder trust`, `profile`. The daemon reads those facts with the
launch's own code (`Sessions::sandbox_preflight`), which launches nothing and writes no file;
`torlek sandbox show` prints every gate. A project's report lists, for each provider the
daemon wired, the gates that are not available, a host's gate once and a provider's with the
provider in its name, or one available `launch` line when every launch passes. `torlek init`
holds its session step on a gate that blocks.

`sandbox` reports the `agents.sandbox` the running daemon applies, which is what it last read
at its start or at a `torlek config reload`. When the owner's configuration now says another
value, the line is `missing-prerequisite`: it names both values, what still happens to agents,
and `torlek config reload` (or a daemon restart) as the next action (#963). A configuration that
does not read has nothing to compare; `config` blocks on it.

Docker is probed only then, by the daemon with `cockpit-containers`: a `docker` program is not
an engine. The engine's ID, name and version identify it, and a rootful engine on Linux is
`missing-prerequisite`: whoever reaches its socket is root on the host (report 11).

Under `agents.sandbox = "required"` sandboxed sessions never reach the owner's engine, so
`docker` reports the project's own instead (#390): rootless Docker under a system user of
its own, which the daemon verifies through its socket against the engine the owner
recorded. Not set up, not recorded and refused are `missing-prerequisite`, each with its
next step (the setup script to review and run, `torlek containers record`, starting the
unit). A verified engine is `available` once Torlek's masks helper's socket is installed for
the owner, and `missing-prerequisite` until then: sessions reach the engine only through it.

Under `agents.sandbox = "required"` each compose file in the project's folder (`compose.yaml`,
`docker-compose.yml` and their overrides) gets a line too (#1371). Parallel sessions share
the project's engine, each under its own `COMPOSE_PROJECT_NAME` and port block, so a fixed
`container_name:`, a fixed volume or network `name:` and a literal host port each get a line
naming the file and line, with the variable form that fixes it (`${TORLEK_PORT_0:-8080}`).
None blocks. The daemon reads each file only as a plain file of the checkout, never through a
symbolic link, and the scan reads lines, not YAML.

Provider sign-in comes from the adapter on each readiness request (#443): signed in is
`available`, explicitly signed out is `needs-authorization` with the provider's next step,
and an unavailable or failed probe is `unknown`. Its detail names the probe's own provenance.
This reports local credentials, not token validity, model access or host permission.

A provider the daemon wires but cannot use (not installed, or failing its version) is
`available` with `not usable, optional` and how to install it, as an unsandboxed owner is: a
host needs one usable provider, not all of them (#411). `providers` blocks when none is
usable, and a project's `workflow preset` blocks when no usable provider reports the
capabilities its steps need.
