# 11. Security, isolation and platforms

Status: proposed; platform enforcement `validate`

## Decision

- **One daemon per OS user**, XDG layout: config in `$XDG_CONFIG_HOME/cockpit`, state in
  `$XDG_STATE_HOME/cockpit`, artifacts in `$XDG_DATA_HOME/cockpit`, sockets and locks in
  `$XDG_RUNTIME_DIR/cockpit` (user-owned, 0700, local, non-persistent). State on local disk only.
- Local clients authenticate by **OS peer identity**; the daemon is Unix-socket-only by
  default. Phone/web access is an explicit, authenticated, revocable transport with scoped
  actions.
- An agent gets **task-scoped authority**, never the owner's full Cockpit authority. Session
  and task ids identify records; they never grant access.
- Isolation is a **verified backend capability**, not a flag: requested, supported, effective
  and observed are reported separately; never fall back to a weaker backend silently.

## Sandbox backends

- **No backend today (#828, owner direction 2026-11-05).** Greywall and Greyproxy were removed:
  nothing used them. The product default of `agents.sandbox` is `off`: agents run unsandboxed,
  with the owner's rights, and the risk is logged at the daemon's start and at every launch;
  `torlek doctor`, `torlek init` and each launch's evidence say "no sandbox". An explicit
  `agents.sandbox = "required"` refuses every launch naming the missing backend; it never runs
  an agent unsandboxed. Until a backend is live an agent can reach the owner socket and act as
  the owner, so grants do not bind it (#729); Theo accepts this while no sandbox is in use.
- **Torlek's own sandbox replaces them** (#546,
  [research](../../research/2026-11-05-546-own-sandbox-and-traffic-control.md)): a per-session
  launcher, `torlek-sandbox`, that builds the agent's namespaces, applies Landlock and seccomp
  from memory and serves one loopback listener as a per-session egress proxy whose unknown
  hosts become owner decisions (#674). Linux and WSL2 first; macOS later. Stage 2 makes it
  usable with an explicit `required`; stage 5 returns the product default to `required`.
- The profile Cockpit requests, its refusals and the launch evidence are backend-neutral
  (`cockpit-sandbox`), so the native backend plugs in behind the same interface. Cockpit keeps
  a **capability profile per platform and backend**; a backend's presence is not evidence that
  a requested policy is enforced: the backend reports what it applied.
- `torlek doctor` reports the removed engines' leftovers (the `torlek-greyproxy-<profile>`
  login service and its state folders); only the owner's `torlek doctor --remove-old-sandbox`
  stops and deletes them.
- Rootless Docker per project: dedicated engine identity, checkout-specific containers,
  volumes, networks and ports. Same-UID separation is not a hostile-tenant boundary.
- Windows: everything runs inside the chosen WSL2 distribution. Keep projects on the Linux
  filesystem for performance; WSL interop can launch Windows executables and reach Windows
  drives, so the sandbox must constrain those paths; WSL itself is not the sandbox. Cockpit
  restart and WSL shutdown are different events; systemd services do not keep WSL alive.

## Trust boundaries (unchanged from today, restated)

- Host approvals bind the effective command, working directory and reviewed content hashes;
  consumed once with the effect intent.
- Evidence APIs (source, CI, runtime) are narrowly scoped, identity-checked, bounded, expiring
  and revocable, so ordinary verification needs no host approval.
- Security audit log stays separate from diagnostics; agent-writable paths are read through
  the untrusted-read rules; sender identity comes from the kernel, never from payloads.
- Plugins, skills, team messages and project config request; `cockpit-authorization` decides.

## Rejected

- Shared global Docker socket; TCP-exposed daemon by default; token passthrough to providers.
- Native Windows sandboxing (Job Objects, AppContainer): out of scope with WSL2-only Windows.

## Sources

- XDG: <https://specifications.freedesktop.org/basedir/latest/>
- Docker rootless and socket protection: <https://docs.docker.com/engine/security/rootless/>,
  <https://docs.docker.com/engine/security/protect-access/>
- Torlek's own sandbox and traffic control, with its sources:
  [#546 research](../../research/2026-11-05-546-own-sandbox-and-traffic-control.md)
- WSL config/interop/networking: <https://learn.microsoft.com/en-us/windows/wsl/wsl-config>,
  <https://learn.microsoft.com/en-us/windows/wsl/networking>
- MCP security best practices (confused deputy, local servers, state handles):
  <https://modelcontextprotocol.io/docs/2026-07-28/tutorials/security/security_best_practices>
