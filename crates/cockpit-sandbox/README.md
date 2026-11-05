# cockpit-sandbox

Owns the sandbox every launched agent runs in: the profile Cockpit requests, the platform it
would be enforced on, the refusal while no backend enforces it and the evidence of what each
launch got.

**This release has no sandbox backend.** Greywall and Greyproxy were removed (#828); Torlek's
own native sandbox (#546, [research](../../docs/research/2026-11-05-546-own-sandbox-and-traffic-control.md))
replaces them in later stages. Until it exists:

- `agents.sandbox = "off"` (the product default) runs every agent unsandboxed, with the
  owner's rights. `Sandbox::prepare` returns the command unchanged with `Unsandboxed`
  evidence, reason `no sandbox: agents.sandbox = "off"`.
- `agents.sandbox = "required"` refuses every launch: `Sandbox::prepare` returns a `Refusal`
  naming the missing backend (`cockpit_config::NO_SANDBOX_BACKEND`). It never degrades to an
  unsandboxed launch.

An unsandboxed agent can reach the owner socket and act as the owner, so Torlek's grants only
hold under a sandbox. Inside a session `torlek` sends every verb on the agent socket, so an
owner verb typed there is refused, not run as the owner (#729); that stops accidents, not a
same-user process that opens the owner socket itself. The daemon's start, every unsandboxed
launch, `torlek doctor` and `torlek init` say so.

## What stays for the native backend

1. `Profile::build` turns what a launch asks for (working directory, the session's own
   directory, read-only mounts the daemon needs the agent to reach, Cockpit's own state it
   must never reach, extra hosts, environment, secrets, the owner's desktop session choice)
   into the effective profile, with Cockpit's baseline rules added. A rule the platform
   cannot enforce is a `Refusal` naming the rule and the reason; nothing is weakened
   silently. `Sandbox::profile` builds it from the host paths the daemon read; no launch
   uses it while no backend enforces it.
2. `ProviderReach` says how the daemon reaches a provider's sockets in a launch: directly
   (every unsandboxed launch), through folders a backend mounts (`FolderMount`, Codex's
   `/tmp/codex-daemon-<uid>`, #446), or not at all, with the reason.
3. `Evidence` says what one launch got: enforced or unsandboxed, the platform, every rule
   with what enforced it, and the host capabilities observed. `record` stores it in the
   launch's transaction; `latest_for_session` reads it back for `torlek session sandbox`.
   Rows an earlier release recorded as `Sandboxed` still read back; this release records
   none.
4. `record_observations` appends one bounded owner-measured batch to the latest sandboxed
   launch (#433): 1–16 unique, nonblank observations of at most 512 bytes each, without
   control characters. Agent callers, stale identities and unsandboxed launches are
   refused; an identical retry is idempotent. Reports never change policy or grants.

Edges: `cockpit-config` (the owner's `DesktopSession` and `AgentSandbox` values and the
no-backend text) and `cockpit-turso` (the evidence table, behind the default `persistence`
feature). `cockpit-sessions` calls it for every launch.

## What the removed engines left

`torlek doctor` reports the removed engines' leftovers in a profile: the
`torlek-greyproxy-<profile>` login service and the `greyproxy`, `engines` and `sandbox`
folders in its state directory. Only the owner's `torlek doctor --remove-old-sandbox` stops
the service and deletes them.
