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
- Escaping the agent sandbox once a backend enforces it, or reaching files, hosts or
  sockets the sandbox profile denies; an `agents.sandbox = "required"` launch that runs
  unsandboxed.
- Bypassing host-side approval, or a request accepted from a sender whose identity was
  not authenticated on the agent channel.
- Another local OS user, or a remote client such as the phone interface, reading or
  controlling the owner's daemon, sessions, credentials or state.
- Secrets or hosted work leaking into Cockpit's diagnostics or support bundle.

Out of scope:

- Anything that already requires the owner's own OS account or root on the machine.
- Shared multi-human control, native Windows execution and other configurations Cockpit
  does not support (Windows runs Cockpit inside WSL2).
- Vulnerabilities in the coding agents themselves: report those to their projects.
- What an agent does with the owner's rights while it runs unsandboxed: that is the
  documented risk of `agents.sandbox = "off"`, not a vulnerability.

## Threat model summary

- **Owner versus agents.** The owner is trusted; agents and everything they produce
  (output, messages, files, proposed commands) are not. An agent reaches the daemon only
  through one authenticated agent channel, and every privileged action is checked against
  grants the owner set.
- **Sandbox.** This release has no sandbox backend: Greywall and Greyproxy were removed
  (#828) and Torlek's own sandbox (#546) replaces them. By default (`agents.sandbox =
  "off"`) agents run unsandboxed, with the owner's rights, so grants do not bind them; the
  daemon logs that risk at its start and at every launch. An explicit `required` refuses
  every launch. Once a backend is live, the sandbox profile decides which paths, hosts and
  commands are reachable, and a gap in the profile is a vulnerability, not a feature.
- **Host actions.** Installs, Git writes and other host commands an agent asks for run
  only after the owner approves them on the host side.
- **Per-user daemon.** The daemon serves its own OS user only and has no desktop
  dependencies; desktop and phone features are separate clients with their own access.

Details live in the design reports for
[security, isolation and platforms](docs/design/greenfield/11-security-isolation-platforms.md)
and [the agent channel and approvals](docs/design/greenfield/17-agent-channel-and-approvals.md).
Those reports describe the target design; parts are still being built under `crates/`.
