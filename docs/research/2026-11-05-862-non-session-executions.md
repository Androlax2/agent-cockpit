# Checks and plugins under the native sandbox

Decision issue [#862](https://github.com/Androlax2/agent-cockpit/issues/862), a decision issue
for the sandbox stage (alpha 4) with no product code. Covers
[#352](https://github.com/Androlax2/agent-cockpit/issues/352) (checks in their declared
environment), [#372](https://github.com/Androlax2/agent-cockpit/issues/372) (plugin egress),
[#370](https://github.com/Androlax2/agent-cockpit/issues/370) (plugin sandbox) and
[#371](https://github.com/Androlax2/agent-cockpit/issues/371) (plugin permissions). Background:
the [native sandbox report](2026-11-05-546-own-sandbox-and-traffic-control.md) and its decisions
D1 to D6, and the sibling decisions for
[#856](2026-11-05-856-docker-in-sandboxed-sessions.md),
[#857](2026-11-05-857-forge-credentials-and-push-policy.md),
[#859](2026-11-05-859-session-folders-and-caches.md) and
[#861](2026-11-05-861-unadapted-programs-and-adopted-sessions.md). **Research and proposal,
2026-11-05; Theo's decisions are recorded under [Decisions](#decisions) as they are made.**
Merging this document accepts research delivery and records those answers; it authorizes no
product code, dependency or host setting, and it edits no issue. The issue texts under
[Changes for each linked issue](#changes-for-each-linked-issue) are proposals; the owner's side
applies them.

## The question

D2 puts the egress proxy in each session's launcher, and D4 scopes "allow always" to a project.
Two kinds of execution are not sessions and are covered by neither:

1. **A check.** The daemon runs `[[checks]]` itself (#352: "in their declared sandbox"), as the
   owner's execution, not an agent's.
2. **A plugin.** #372 gives plugins "the same proxy model agents use", but a plugin has no
   launcher, no checkout and no project of its own.

Four things to settle:

- **Q1.** Does a daemon-run check launch through `torlek-sandbox`?
- **Q2.** If so, with which hosts, folders and credentials?
- **Q3.** Do plugins get network access outside sessions, and through which path?
- **Q4.** Is a plugin a grant subject next to the project in #674's schema?

The container part of #352 is already settled by #856 (a `docker` check runs in a container of
the project's Torlek-run engine, never the owner's) and is not reopened.

## What exists today

Code at this branch's base (`93646e8f`); paths are relative to the repository root.

| Where | Fact |
| --- | --- |
| `crates/cockpit-daemon/src/verification.rs:396`, `:36` | `check run` refuses every check whose environment is not `native`, before anything starts |
| `verification.rs:482-493` | A check is `/bin/sh -c <command>` in the checkout, started through `launch_reserved_with_output` as an execution of the daemon's supervisor (the run id is the execution's, #29) |
| `crates/cockpit-daemon/src/serve.rs:96`, `crates/cockpit-approvals/src/pty.rs:554` | The check's environment is the daemon's whole environment, minus `GIT_*` and the three `TORLEK_*` session variables. Nothing else is withheld: an owner token set in the daemon's environment (for example `GH_TOKEN`) is in it |
| `verification.rs:249`, `crates/cockpit-daemon/src/routes.rs:570-580` | Only the owner runs a check, and only in a trusted project; an untrusted or review checkout is refused (#649, #676). An agent can only claim a result |
| `crates/cockpit-config/src/lib.rs:254`, `:1053` | A check's `command` and `environment` come from the project's `.torlek/config.toml`; only the project file declares checks. `.torlek` is in `BASELINE_DENIED_WRITE`, so a session cannot edit it, but a merged change can |
| `crates/cockpit-config/src/lib.rs:167-181` | `ExecutionEnvironment::{Native, Sandbox, Docker}` exists as the project's *requirement*: "a project that expects `native` work still runs sandboxed until the owner turns the sandbox off" |
| `crates/cockpit-processes/src/systemd.rs` | Executions already run in transient systemd user scopes (#29), the placement #829's comment asks the launcher to keep for #834 |
| #859 report, "What exists today" | The owner's `make test` or a #352 check "runs agent-chosen code" on the host, with the owner's `~/.cargo` and sccache; sessions now get caches of their own, so only the host side still shares them |
| #313 / #352 text | A check's receipt must record the environment that actually ran; a missing environment is `unavailable`, not refused silently |
| `docs/design/greenfield/14-plugins.md` | Plugins are external-process over the protocol first, WASM later; "a plugin can never widen authority; it requests, Cockpit decides" |
| #370, #372 text | A WASM plugin has no `wasi:sockets`; network is "a host interface (an HTTP client the host performs), never raw sockets", allowlist from the manifest, DNS resolved by the host, no redirects off the allowlist |
| #371 text | The manifest's permissions map onto `cockpit-authorization` grants; consent at install, new permission asks again, per-project scoping, revocation immediate, "network to declared hosts only (high)" |

## Threats

The adversary is the one in the #546 threat model: the agent and the content it reads, working
as the owner's OS user. A check adds that its input is the agent's output.

1. **Agent-written code runs outside the sandbox.** A session writes a test, a `build.rs`, a
   `postinstall` script or a Makefile target inside its sandbox. The owner then runs `check run`
   and the daemon executes it as the owner with no namespaces, Landlock or seccomp. Every
   mask of #829 stops mattering for any project with a check.
2. **The owner's environment and credentials.** The check holds the daemon's environment and
   can read `~/.ssh`, `~/.aws`, `~/.config/gh` and the state database.
3. **The owner socket.** A check can reach the host-only socket and act as the owner (#729), so
   no grant binds it.
4. **Unbounded network.** A check can reach any host, the LAN and cloud metadata.
5. **Shared host caches.** A check reads and writes the owner's `~/.cargo`, `~/.npm` and
   sccache, which #859 keeps out of sessions; a poisoned entry then reaches every other build.
6. **A pass is not truer for being sandboxed.** The receipt records an exit code and the
   fingerprint of the files taken before the run; agent code can exit 0 either way. A sandbox
   bounds what the code can do, not whether the result is honest. Out of scope here.
7. **Silent fallback.** A check that cannot start in its sandbox must not run on the host
   instead; the receipt names the environment that ran (#352).
8. **Plugin exfiltration.** A plugin that reads pane contents or todo text and can call out
   is a channel for them (#372, #371).
9. **Owner fatigue and request text.** More subjects that file network requests means more
   decisions and more text the requester might shape (#861 threat 4).
10. **The daemon as a network client for untrusted parties.** A host-performed plugin request
    parses a response from a host the plugin chose, in the process that holds the owner's state
    (D2's reason against putting the proxy in the daemon, M7 B).

## Options

### Q1. Does a daemon-run check launch through `torlek-sandbox`

| Option | Threats it leaves | Cost |
| --- | --- | --- |
| **A. No: checks stay `native`, on the host** (today) | 1 to 5 all open for every check; the sandbox protects the agent's session and the owner then runs the agent's code outside it | None |
| **B. Yes, following the owner's switch** (recommended). Under `agents.sandbox = "required"` every check runs through `torlek-sandbox`; `environment = "sandbox"` is the default. A check that declares `native` is recorded `unavailable` with the reason, never run on the host. `agents.sandbox = "off"` keeps today's behaviour, the one named opt-out, with the risk logged | 6 and 8 are unchanged. 5 is closed for the check's own caches. If the sandbox cannot start the check is `unavailable` (7) | A project with a check that needs the host (a display, a system service) gets no receipt for it under `required` until the owner turns the sandbox off or the check moves to `docker` (#856). Every run starts with an empty cache of its own: dependencies are fetched and built again, not measured here |
| **C. Yes, per check.** `sandbox` is one more environment and the project file chooses; `native` keeps working under `required` for a trusted project | 1 to 5 stay open for every `native` check. The project file picks, so a merged change can switch a check back to the host | Least friction; two paths to keep, and the guarantee is only as good as the project file |

B follows a rule the config already has: the project states a requirement, the owner's switch
decides (`lib.rs:167`). The launcher already takes a `Needs` (#861) and a program, so a check is
the shell as the program and the checkout as its folder; the execution stays the daemon
supervisor's (#352).

### Q2. With what a sandboxed check runs

Assumes Q1 B or C. Folders follow #859 and #861: the checkout writable except `BASELINE_DENIED_WRITE`,
toolchain roots read-only from #830's table, caches in a temp folder of the execution, removed
when it ends. The open part is the network and credentials.

| Option | Threats it leaves | Cost |
| --- | --- | --- |
| **A. No network** | 4 closed, 9 none | Caches start empty, so a check that needs a dependency fails unless the project vendors it. In practice most checks cannot run |
| **B. The project's grants plus the detected ecosystem's registry hosts** (recommended). The same allowlist sources as a session, minus a provider's hosts: a check never reaches the model API. An unknown host is refused at once and filed as a #674 request whose subject is the check (allow once, allow always for the project, deny). No bound credential (#832) for a check in this release | 4 reduced to the hosts the owner already granted the project; registries stay a read channel (#859 residual) | An owner decision when a check needs a host no session did. "Allow once" is meaningful because the run ends |
| **C. Everything a session of the project reaches**, including the provider's hosts | 4 reduced to the session's allowlist, which includes the model API | None extra; a check has no reason to call a model |

A check declares hosts in the project file only as pre-filled requests, never as grants
(#861 Q1).

### Q3. Do plugins reach the network, and through which path

| Option | Threats it leaves | Cost |
| --- | --- | --- |
| **A. No plugin network in the first plugin release.** The vocabulary has no network permission; #372 waits | 8 and 10 closed | Plugins cannot call a webhook or an API at all |
| **B. The plugin host performs the request in the daemon**, through the decision core of #831 used as a library (allowlist snapshot plus subject), no listener | 8 reduced to the declared hosts and best-effort taint (#372). 10 open: untrusted responses are parsed in the control process; one mistake in the core is the daemon reaching loopback or the LAN | No extra process |
| **C. The plugin host sends the request to an egress-only instance of the launcher** (recommended): the same decision core and resolver rules as #831, confined with Landlock and seccomp, with no state, no owner socket and no namespace setup; the daemon never opens a plugin's connection | 8 reduced as in B. 10 closed: parsing and resolution happen in a confined process; a bug there reaches only the resolver files and the network it is granted | A second mode of the launcher and a request protocol to it; about a day more than B |

An external-process plugin, if one ships before WASM (report 14), is an unadapted program
(#861): launched through `torlek-sandbox` with the hosts the owner consented to at install as
its owner entry, so it needs no rule of its own.

### Q4. Is a plugin a grant subject

| Option | Threats it leaves | Cost |
| --- | --- | --- |
| **A. Yes, a subject of its own** (recommended). Grants carry a subject: the project (sessions and checks), the project's containers (#856) or a plugin. A plugin's hosts are granted at install consent and again when an update adds one (#371); optionally scoped to a project. A plugin never inherits the project's hosts, and no session or check inherits a plugin's. A plugin has **no runtime request path**: an undeclared host is refused and counted, and the manifest is the only way to add it | 8 bounded by what the owner consented to. 9 closed for plugins | One more subject value in the schema and the feed event |
| **B. No: a plugin uses the project's grants** | 8 open to every host any session of the project reaches (a forge, registries) | No schema change |
| **C. Plugin hosts in a separate store** under #371's own table | 8 as in A | Two host stores and two revocation and audit paths that can drift (#857 decision 3 wants one host record) |

## Recommendation

Q1 **B**, Q2 **B**, Q3 **C**, Q4 **A**. Checks follow the sandbox switch, get the project's network
without the model API, and file requests like a session; plugins get a confined egress path
and grants of their own that only the manifest and the owner's consent can change.

## Decisions

Theo's answers, asked one at a time in his session.

- [x] **Q1.** Decided 2026-11-05: **B**. Checks follow `agents.sandbox`. Under `required` every
  check runs through `torlek-sandbox`; a check that declares `native` is recorded `unavailable`,
  never run on the host; `off` keeps today's host execution.
- [x] **Q2.** Decided 2026-11-05: **B**. A sandboxed check gets the project's grants plus the
  detected ecosystem's registry hosts, never a provider's hosts. An unknown host is refused at
  once and filed as a #674 request whose subject is the check (allow once, allow always for the
  project, deny). A check gets no bound credential in this release.
- [x] **Q3.** Decided 2026-11-05: **C**. A plugin's network requests go from the plugin host to
  an egress-only instance of the launcher, confined with Landlock and seccomp, with no state and
  no owner socket. The daemon never opens a plugin's connection.
- [x] **Q4.** Decided 2026-11-05: **A**. A plugin is a grant subject of its own. Its hosts are
  granted at install consent and again when an update adds one; it never inherits the project's
  hosts, and no session or check inherits a plugin's; it has no runtime request path.

All four follow the recommendation. What falls out of them: every execution Torlek starts for
the owner runs under the same launcher and the same grant store, whatever its subject (session,
check, containers, plugin), and the only way out of the sandbox for any of them is the one named
opt-out, `agents.sandbox = "off"`.

### Residual risks the decisions accept

- Every sandboxed check run starts with an empty cache of its own and fetches and builds its
  dependencies again. Not measured here. The later optimization is #859's: a read-only lower
  layer limited to a Torlek-managed per-project folder, never the owner's.
- Registries stay a read channel for a check (#859's residual), counted per host.
- A check that needs the host (a display, a system service) has no receipt under `required`
  until its owner moves it to `docker` (#856) or turns the sandbox off.
- A sandboxed check's pass is as honest as the code it ran (threat 6); the receipt still records
  an exit code and a fingerprint, nothing more.
- Under `agents.sandbox = "off"`, checks run on the host with the daemon's environment as today
  (threats 1 to 5), with the same logged risk as unsandboxed sessions.

## Changes for each linked issue

Proposed text; the owner applies it. Nothing here edits an issue. Common to all: a network
request, a grant and a feed event carry a **subject**: a session, a check run, a project's
containers (#856) or a plugin. The decision core of #831 takes an allowlist snapshot and a
subject and nothing session-only.

### #352 Verification: run checks in their declared sandbox or container environment

Replace the Scope's first bullet and add to Acceptance:

> - **Checks follow `agents.sandbox` (#862, Q1).** Under `required`, every check runs through
>   `torlek-sandbox`, started as an execution of the daemon's supervisor so the run id stays the
>   execution's. `environment = "sandbox"` is the default of a check that declares none. A check
>   that declares `native` is recorded `unavailable` with the reason ("native checks run only with
>   `agents.sandbox = "off"`"); it is never run on the host and never refused silently. Under
>   `off`, checks run on the host as today. `docker` is unchanged (#856).
> - **What a sandboxed check gets (#862, Q2).** The checkout writable except
>   `BASELINE_DENIED_WRITE`; the toolchain roots of #830's table read-only; package and tool
>   caches in a folder of the execution, removed when it ends (#859); the environment Torlek
>   builds, not the daemon's. Hosts: the project's grants and the detected ecosystem's registry
>   hosts, never a provider's hosts. An unknown host is refused at once and filed as a #674
>   request whose subject is the check. No bound credential (#832). A check's declared hosts are
>   pre-filled requests and grant nothing (#861).
> - The receipt records the environment that ran and the sandbox evidence (backend, Landlock ABI).
>   A sandbox that cannot start is `unavailable`, not a host run. An untrusted or review
>   checkout is still refused before anything starts (#649, #676).
>
> **Acceptance to add:** through the daemon with `agents.sandbox = "required"` and a fixture
> check that tries each of these, the check cannot read `~/.ssh/id_*` or the state database,
> cannot reach the owner socket, does not see the daemon's variables (a `GH_TOKEN` set in the
> daemon's environment is absent), reaches a granted host and not another, and writes nothing
> into the owner's `~/.cargo`; a `native` check under `required` is recorded `unavailable`; the
> same checks under `off` run on the host. **Depends on:** add #829, #830, #831, #674, #862.

### #829 native Linux sandbox launcher

Add to Acceptance:

> - The launch spec accepts a **check** as its program (#862): the check shell and command, the
>   checkout, an execution folder for caches, the `Needs` of the check, and no provider, agent
>   socket, launch token or session identity. The environment is the one Torlek builds; none of
>   the daemon's variables enter it. The evidence names the subject (a check run) and its
>   backend and Landlock ABI. The execution's systemd scope placement is the same as a session's
>   (#834).

### #831 per-session egress proxy in the session launcher

Add:

> The decision core's input is an allowlist snapshot and a subject (#856 already names a
> project's containers; #862 adds a check run and a plugin). A check's listener is a session-style
> listener with no provider hosts, and its counters and refusals name the check run. A plugin's
> egress is not a `CONNECT` listener: it is an egress-only mode of the launcher built with the
> plugin runtime (#372), which shares this decision core and its resolver and address rules, so
> nothing here is session-only.

### #832 selective TLS interception for hosts with a bound credential

Add:

> A check run holds no bound credential in this release (#862, Q2): from a check, a host with
> a bound credential is a plain tunnel, as from a container (#856). The egress-only instance for
> plugins never intercepts and holds no credential.

### #674 Sandbox network: refuse a request to a host not yet allowed at once, and let the owner decide it

Add:

> **Subjects (#862).** A request names its subject. A **check run** files the same request as a
> session; its choices are allow once, allow always for the project (D4) or deny. Allow once
> is meaningful because the run ends. A **plugin** files none: its hosts are its own grants,
> given at install and update consent (#371), never created by a request decision, so there is
> no prompt a plugin can trigger and no text of its own in front of the owner. A project's grants
> never apply to a plugin and a plugin's never to a session or check. The feed event carries the
> subject kind, its id and the project, so the phone (#245) can show which one asked.

### #372 Plugin network egress: off by default, declared hosts through the proxy, no exfiltration channels

Replace "through the same proxy model agents use (greyproxy)" in Owns, and the first Scope
bullet, with:

> **Path (#862, Q3).** The plugin host never opens a connection in the daemon. A request from a
> plugin goes to an egress-only instance of the native launcher, confined with Landlock and
> seccomp, with no state, no owner socket and no namespace setup for an agent. It shares #831's
> decision core: it resolves names itself, refuses loopback, private, link-local and metadata
> addresses unless granted by address, allows no redirect off the allowlist, applies the size and
> rate limits below, and returns refusals with a reason. Allowlist: the plugin's own grants
> (below), never the project's.
> **Grants (#862, Q4).** The plugin is a grant subject (#371). A host is granted at install
> consent and again whenever an update adds one. A plugin has no runtime request path: an
> undeclared host is refused, counted and shown in diagnostics; only the manifest, through
> consent, adds one. Every request is audited with the subject, host and size; revoking the grant
> closes open requests at once.

Add to Acceptance: "A plugin cannot reach a host the project's sessions can; a granted host is
not reachable from a session; an undeclared host is refused and creates no request for the
owner; the daemon process holds no socket to a plugin's host (checked in the test); revoking the
grant ends an open request." **Depends on:** add #831, #674, #862.

### #370 Plugin sandbox: WASM with no ambient authority

Add to Scope:

> The network host interface (#372) is the only way out: the module has no `wasi:sockets`, and
> the interface hands requests to the confined egress instance, never to a client in the daemon
> (#862, Q3). Add to the malicious-plugin suite: a request to a loopback, private or metadata
> address, and a request to a host only the project's sessions are granted, are refused.

### #371 Plugin permissions: declared grants, owner consent, least privilege, revocation

Replace the "network to declared hosts only (high)" item with:

> network to the hosts the manifest declares (high): a grant whose subject is the plugin, with
> host and port terms, shown at consent with every host listed; an update that adds a host asks
> again and is refused until consent; optional per-project scope; revocation takes effect at
> once and closes open requests. The plugin does not inherit the project's hosts and cannot
> request a host at runtime (#862, Q4). Stored and audited with the same grant records and
> revocation path as session and project grants.

### #70 Plugin runtime

No text change. #862 adds nothing to its scope; #372 and #371 carry the network and grants.

### Not changed

- Issue #856: its text for #352's container part stands and now also depends on this decision.
- Issue #830: its toolchain table also serves a check run; no text change.
- Issue #833: the default-on rule covers checks through #352; its probe launch need not run a
  check.
- Issue #834: a check execution already sits in its own scope (#29); whether checks get
  limits is left to #834.
- Approved commands (`ask run`, #313's neighbours) run on the host after the owner approves each
  command; this report does not change them.
- Trust: an untrusted or review checkout still refuses `check run` before anything starts.
  Whether a sandboxed check could run in an untrusted project is not decided here.
