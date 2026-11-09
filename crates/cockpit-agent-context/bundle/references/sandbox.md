# Sandbox, network and Docker

Torlek may run you inside a sandbox the owner configured. Its limits are deliberate.

- **Denied path or command.** Do not probe for a way around it. If the task needs it, report
  it with `torlek todo blocked TITLE --why TEXT` and tell the owner.
- **Blocked network host.** The session proxy refuses it at once with `403`. curl and git
  print only `CONNECT tunnel failed, response 403`; the status line says why:
  `curl -sv https://HOST -o /dev/null 2>&1 | grep '^< HTTP'`. When it says the owner is
  asked, the host is already in their list as a network request, numbered from your next
  try a second later: tell the owner the host, why you need it and the request number
  (`torlek network allow N` is theirs to run), then retry after they allow it. A denial
  stays a denial. Never tunnel, proxy or mirror around the block.
- **Host commands.** Anything that must run outside the sandbox (installs, system services,
  the owner's own programs) goes through `torlek ask run`.
- **Docker.** Use only the engine your environment provides (`DOCKER_HOST` when set), never
  the owner's main one. Without one, say so and ask; do not start a daemon yourself.
- **Secrets.** Never print, copy or commit credentials. Use the references the environment
  gives you.

## What you will see

| What you see | What it means | The one action |
| --- | --- | --- |
| `~` holds no config, caches or tools from an earlier session | The session home starts empty and is removed when the session ends. | Keep what must survive in the checkout, not in `~`. |
| `Read-only file system` (`EROFS`) writing `.git/config`, `.git/hooks`, `.torlek` or `.cockpit` | These paths are read-only on purpose: Git hooks and config run on the owner's host. | Do not retry. `torlek todo blocked TITLE --why TEXT`, naming the path. |
| A `.env` file reads as empty or is missing | `.env` files are hidden on purpose. | Use the references the environment gives you. If the task needs the values, `torlek todo blocked TITLE --why TEXT`. |
| `command not found` for a tool in `~/.local/bin`, or in another folder outside the toolchain table | That folder is not mounted, so the tool does not exist inside the session. | `torlek todo blocked TITLE --why TEXT`, naming the tool. |
| `git push` is refused | Pushes go to the owner. | Stop pushing. `torlek todo blocked TITLE --why TEXT`, naming the branch. |

A `403` from the session proxy is a blocked host, not a missing tool: see the network rule above.

`TORLEK_SESSION` names your session and `TORLEK_INSTRUCTIONS` the directory holding this
manual as written by the running Torlek. Leave both unchanged.
