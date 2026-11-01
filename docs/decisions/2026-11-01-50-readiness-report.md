# Readiness is a pure crate fed facts by the daemon, and `cockpit doctor` blocks only on what must be fixed first

Issue #50. Decided by the implementing session, within report 09 (statuses and next actions)
and report 11 (repository configuration describes, owner settings grant).

- **New crate `cockpit-readiness`, allowed to import `cockpit-config` only.** It turns facts
  into one `Requirement` per need (status, detail, next action). It runs nothing except
  `docker info`. The daemon gathers every fact from the crate that owns it (tmux and the
  sandbox engine from its start, providers from its wiring, controls and capacity from
  `cockpit-resource-admission`, trust from `cockpit-projects`, presets from
  `cockpit-workflows`). That way readiness never makes a second decision those owners already
  make. The rule `readiness -> config only` is in
  `crates/cockpit-workspace-rules/tests/crate_graph.rs`. The same pattern serves agent
  context (#49).
- **`cockpit doctor` is served by the daemon.** The CLI may import only client, protocol and
  config. Adding readiness to that list would be a second decision. Without a daemon the CLI
  prints what it can read alone: the configuration, `daemon: missing-prerequisite` with how
  to start it, and the host as `unknown`. It never prints a guessed host report.
- **Exit 1 only when a requirement blocks** (`missing-prerequisite` or `unsupported`).
  `needs-authorization` is the owner's choice, not a fault: an untrusted project, or a project
  expecting the sandbox while `agents.sandbox = "off"`. `unknown` is what Cockpit cannot tell
  yet. Failing on either would make the command fail on hosts that work.
- **Unsandboxed is `available`, and says so loudly.** `agents.sandbox = "off"` is the owner's
  grant (#61). The detail reads `agents run UNSANDBOXED` and the next action names how to turn
  the sandbox back on.
- **Resource controls are always `available`.** Admission works on every host. The detail
  names what is enforced and what is only requested (report 10). Blocking on a missing cgroup
  controller would fail every macOS host.
- **A rootful Docker engine blocks on Linux and WSL2.** Whoever reaches its socket is root on
  the host, and report 11 requires rootless. Docker Desktop on macOS runs inside its own Linux
  VM, so it is `available` there. Docker is probed only for a project that declares
  `[execution] environment = "docker"` or a docker check, within 10 seconds. The probe moves
  to `cockpit-containers` when that crate lands (#390).
- **Project requirements are describing settings:** `[execution] environment`
  (`native|sandbox|docker`), `[needs] cpu_millis/memory_mib`, `[results] artifacts`. They are
  refused in the owner file, just as `[[checks]]` is. Needs above the host's budget are
  `unsupported`.
- **The capabilities a project needs come from its preset's steps,** not from a new config
  key, so a workflow and its readiness cannot disagree.
- **Provider sign-in is `unknown`** until each adapter can report it (#443). Starting the
  agent to find out would cost the owner a session and could open a browser.
- **Rejected:** a readiness check inside each owner crate (`fn readiness()` on sessions,
  sandbox and so on). The statuses and next actions would be spread over a dozen crates, and
  the report's wording could not be reviewed in one place.
- **Rejected:** a CLI-side report that runs tmux, greywall and claude itself. It would duplicate
  the daemon's discovery and drift from what the daemon actually launches with.
