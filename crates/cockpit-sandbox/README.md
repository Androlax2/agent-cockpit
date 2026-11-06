# cockpit-sandbox

Owns the sandbox every launched agent runs in: the profile Cockpit requests, the launch spec
the native launcher enforces, the refusal on a host without it and the evidence of what each
launch got.

**The backend is Torlek's native sandbox** (#829, [research](../../docs/research/2026-11-05-546-own-sandbox-and-traffic-control.md)):
`torlek-sandbox` ([its crate](../cockpit-sandbox-launcher/README.md)) under bubblewrap, with
Landlock and seccomp. The daemon probes it once at its start (`Backend::probe`):

- `agents.sandbox = "off"` (the product default) runs every agent unsandboxed, with the
  owner's rights. `Sandbox::prepare` returns the command unchanged with `Unsandboxed`
  evidence, reason `no sandbox: agents.sandbox = "off"`.
- `agents.sandbox = "required"` with the native backend: `Sandbox::prepare` builds the
  profile, then the `LaunchSpec` (`spec.rs`) the launcher reads, and returns the launcher in
  the agent's place with `Sandboxed` evidence naming bubblewrap's version and the Landlock
  ABI. `Staged::write` is the one place a sandboxed session's mount sources and own home are
  created. The provider's declared credential variables are dropped and its credential files
  masked (#858); the network is the session proxy's (#831, `egress.rs`). A placeholder
  secret or a shared desktop session is refused.
- Bound credentials (#832, `credentials.rs`): a `LaunchRequest`'s `credentials` name their
  hosts and schemes in the spec's `interception` and their values in `credentials.json`,
  owner-only beside the spec, which the launcher reads and removes. The agent gets each
  credential's placeholder variable and the trust variables that name the session CA's files
  in the tool configuration folder. A host the allowlist does not let through on 443 cannot
  be bound, and an unsandboxed launch is refused any credential.
- Toolchains and caches (#859): the caller resolves `ToolNeeds` from the toolchain table of
  `cockpit-provider-api`. The install roots `PATH` resolves the checkout's ecosystems in are
  bound read-only; every cache variable points into `caches/` of the session's temp folder;
  tool configuration files, the session's Git configuration among them, are written by
  `Staged::write` into a folder of the mount sources bound read-only, which the session's
  end removes with the rest of its mount sources (`Sandbox::session_mount_sources`, #482). Git reads no system configuration and no
  `GIT_CONFIG*` variable of the launch. Nothing of the owner's caches or tool configuration
  is mounted.
- Declarations (#861): the evidence names where the program's needs came from, its adapter,
  the owner's entry for it, or none (the floor). `cockpit-sessions` refuses, when the daemon
  loads its providers, a declaration that reads or writes a baseline-denied path or another
  program's `write` folder.
- Checks (#862): `Sandbox::prepare_check` gives a project's check the same launcher with
  the check run as its subject, and no provider, agent socket, bridge program, launch token
  or session identity.
- `agents.sandbox = "required"` on a host without it (no `bwrap`, a failed probe, Landlock
  below ABI 2): `Sandbox::prepare` returns a `Refusal` naming why. It never degrades to an
  unsandboxed launch.

An unsandboxed agent can reach the owner socket and act as the owner, so Torlek's grants only
hold under a sandbox. Inside a session `torlek` sends every verb on the agent socket, so an
owner verb typed there is refused, not run as the owner (#729); that stops accidents, not a
same-user process that opens the owner socket itself. The daemon's start, every unsandboxed
launch, `torlek doctor` and `torlek init` say so.

## Profile, reach and evidence

1. `Profile::build` turns what a launch asks for (working directory, the session's own
   directory, read-only mounts the daemon needs the agent to reach, Cockpit's own state it
   must never reach, extra hosts, environment, secrets, the owner's desktop session choice)
   into the effective profile, with Cockpit's baseline rules added. A rule the platform
   cannot enforce is a `Refusal` naming the rule and the reason; nothing is weakened
   silently. `Sandbox::profile` builds it from the host paths the daemon read.
2. `ProviderReach` says how the daemon reaches a provider's sockets in a launch: directly
   (every unsandboxed launch), through folders a backend mounts (`FolderMount`, Codex's
   `/tmp/codex-daemon-<uid>`, #446), or not at all, with the reason.
3. `Evidence` says what one launch got: enforced or unsandboxed, the platform, every rule
   with what enforced it, and the host capabilities observed. `record` stores it in the
   launch's transaction; `latest_for_session` reads it back for `torlek session sandbox`.
   Rows an earlier release recorded as `Sandboxed` still read back.
4. `record_observations` appends one bounded owner-measured batch to the latest sandboxed
   launch (#433): 1–16 unique, nonblank observations of at most 512 bytes each, without
   control characters. Agent callers, stale identities and unsandboxed launches are
   refused; an identical retry is idempotent. Reports never change policy or grants.
5. Network requests (#674, `network.rs`, migration 2): `file_network_request` files a
   destination the launcher refused for want of an entry, once per session, host and port,
   unless a decision or a grant already covers it. The owner's `allow_network_request`
   (for the rest of the session, or `always`: a project grant for that host and port),
   `deny_network_request` and `revoke_network_grant` record who decided, when and through
   which client; an allow within `NETWORK_ALLOW_GRACE_SECONDS` of the request is refused.
   `egress_decisions` is what the session's launcher is given: `Staged::write` writes it
   as `egress-decisions.json`, with the requests still asking, which allow nothing; the
   daemon rewrites it after each decision and each request it files.

Edges: `cockpit-config` (the owner's `DesktopSession` and `AgentSandbox` values) and `cockpit-turso` (the evidence and network request tables, behind the default `persistence`
feature). `cockpit-sessions` calls it for every launch.

## What the removed engines left

`torlek doctor` reports the removed engines' leftovers in a profile: the
`torlek-greyproxy-<profile>` login service and the `greyproxy`, `engines` and `sandbox`
folders in its state directory. Only the owner's `torlek doctor --remove-old-sandbox` stops
the service and deletes them.
