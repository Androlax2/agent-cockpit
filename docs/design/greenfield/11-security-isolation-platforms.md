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

- Greywall covers Linux (bubblewrap, Landlock, seccomp, network namespace + TUN) and macOS
  (Seatbelt; proxy by environment variables only, no full traffic capture, `.env` files denied
  rather than rewritten). Greywall is defense in depth, not hostile-code containment.
- Cockpit keeps a **capability profile per platform and backend**; "Greywall installed" is not
  evidence that a requested policy is enforced.
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
- Greywall platform support and security model: <https://docs.greywall.io/greywall/platform-support>,
  <https://docs.greywall.io/greywall/security-model>
- WSL config/interop/networking: <https://learn.microsoft.com/en-us/windows/wsl/wsl-config>,
  <https://learn.microsoft.com/en-us/windows/wsl/networking>
- MCP security best practices (confused deputy, local servers, state handles):
  <https://modelcontextprotocol.io/docs/2026-07-28/tutorials/security/security_best_practices>
