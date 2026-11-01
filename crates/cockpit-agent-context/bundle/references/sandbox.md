# Sandbox, network and Docker

Cockpit may run you inside a sandbox the owner configured. Its limits are deliberate.

- **Denied path or command.** Do not probe for a way around it. If the task needs it, report
  it with `cockpit todo blocked TITLE --why TEXT` and tell the owner.
- **Blocked network host.** Name the host and ask the owner to allow it; never tunnel,
  proxy or mirror around the block.
- **Host commands.** Anything that must run outside the sandbox (installs, system services,
  the owner's own programs) goes through `cockpit ask run`.
- **Docker.** Use only the engine your environment provides (`DOCKER_HOST` when set), never
  the owner's main one. Without one, say so and ask; do not start a daemon yourself.
- **Secrets.** Never print, copy or commit credentials. Use the references the environment
  gives you.

`COCKPIT_SESSION` names your session and `COCKPIT_INSTRUCTIONS` the directory holding this
manual as written by the running Cockpit. Leave both unchanged.
