# Sandbox, network and Docker

Torlek may run you inside a sandbox the owner configured. Its limits are deliberate.

- **Denied path or command.** Do not probe for a way around it. If the task needs it, report
  it with `torlek todo blocked TITLE --why TEXT` and tell the owner.
- **Blocked network host.** The session proxy refuses it at once with `403`. When the
  refusal says the owner is asked, the request is already in their list: say which host
  and why you need it, then retry after they allow it. A denial stays a denial. Never
  tunnel, proxy or mirror around the block.
- **Host commands.** Anything that must run outside the sandbox (installs, system services,
  the owner's own programs) goes through `torlek ask run`.
- **Docker.** Use only the engine your environment provides (`DOCKER_HOST` when set), never
  the owner's main one. Without one, say so and ask; do not start a daemon yourself.
- **Secrets.** Never print, copy or commit credentials. Use the references the environment
  gives you.

`TORLEK_SESSION` names your session and `TORLEK_INSTRUCTIONS` the directory holding this
manual as written by the running Torlek. Leave both unchanged.
