# Security policy

## Reporting a vulnerability

Report vulnerabilities privately through GitHub:
**Security → Report a vulnerability** on this repository
([direct link](https://github.com/Androlax2/agent-cockpit/security/advisories/new)).
Do not open a public issue, pull request or discussion for a vulnerability.

Include what an attacker controls, what they gain, the steps or a proof of concept, and the
commit you tested. Cockpit is maintained by one person, so responses are best effort.
Fixes land on `main`; the advisory is published once the fix is merged.

## Supported versions

There are no releases yet. Only the latest commit on `main` is supported.

## Scope

Cockpit is a personal installation: one human owner, one daemon per OS user, many clients
([scope](docs/design/greenfield/01-scope-and-principles.md)). It runs coding agents, which
it treats as untrusted. In scope:

- An agent, a plugin, a skill or any other prose gaining authority the owner did not
  grant. Instructions request; only `cockpit-authorization` decides.
- Escaping the agent sandbox (greywall and greyproxy, which Cockpit ships as its own
  pinned forks), or reaching files, hosts or sockets the sandbox profile denies.
- Bypassing host-side approval, or a request accepted from a sender whose identity was
  not authenticated on the agent channel.
- Another local OS user, or a remote client such as the phone interface, reading or
  controlling the owner's daemon, sessions, credentials or state.
- Secrets or hosted work leaking into Cockpit's diagnostics or support bundle.

Out of scope:

- Anything that already requires the owner's own OS account or root on the machine.
- Shared multi-human control, native Windows execution and other configurations Cockpit
  does not support (Windows runs Cockpit inside WSL2).
- Vulnerabilities in the coding agents themselves, or in upstream greywall and greyproxy
  that do not affect the forks Cockpit pins: report those to their projects.

## Threat model summary

- **Owner versus agents.** The owner is trusted; agents and everything they produce
  (output, messages, files, proposed commands) are not. An agent reaches the daemon only
  through one authenticated agent channel, and every privileged action is checked against
  grants the owner set.
- **Sandbox.** Each agent session runs inside greywall, with network traffic through
  greyproxy. The sandbox profile decides which paths, hosts and commands are reachable;
  a gap in the profile is a vulnerability, not a feature.
- **Host actions.** Installs, Git writes and other host commands an agent asks for run
  only after the owner approves them on the host side.
- **Per-user daemon.** The daemon serves its own OS user only and has no desktop
  dependencies; desktop and phone features are separate clients with their own access.

Details live in the design reports for
[security, isolation and platforms](docs/design/greenfield/11-security-isolation-platforms.md)
and [the agent channel and approvals](docs/design/greenfield/17-agent-channel-and-approvals.md).
Those reports describe the target design; parts are still being built under `crates/`.
