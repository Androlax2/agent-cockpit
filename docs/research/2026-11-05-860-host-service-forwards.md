# How a sandboxed session reaches an owner-approved service on the host

Decision issue [#860](https://github.com/Androlax2/agent-cockpit/issues/860), made at the
sandbox stage by Theo's rule of 2026-11-05. It builds on the
[native sandbox plan](2026-11-05-546-own-sandbox-and-traffic-control.md) (decisions cited as
"#546 D1" to "#546 D6"), the [Docker decision](2026-11-05-856-docker-in-sandboxed-sessions.md)
(#856, which leaves container port forwards to this issue) and the
[forge decision](2026-11-05-857-forge-credentials-and-push-policy.md) (#857, host records).
**Research and proposal; Theo decided all four questions on 2026-11-05, recorded under
[Decisions for the owner](#decisions-for-the-owner).** Merging this document
accepts research delivery only: it authorizes no product code, dependency, host setting or
issue edit. The [issue texts](#changes-to-the-linked-issues-proposed-text) at the end are
proposals; Theo applies them.

## The question

A session in the native sandbox has an empty network namespace and one proxy listener
(#546 M4 A, #831); #831 refuses loopback. A service on the host (FigCraft's control port for
#71, an MCP server, a dev server, a container's published port from #390) is therefore
unreachable. Is there a way in, what does it carry, and who decides?

Only the direction session to host is covered. A service the agent starts itself listens on the
namespace's own loopback, which `NO_PROXY=localhost,127.0.0.1,::1` already keeps direct
(#546 decision 2). The owner's browser reaching a server inside a session is not asked for here.

## What exists today

| Where | Fact |
| --- | --- |
| `config/bin/.local/bin/greywall-agents/launcher-common.sh:431`, `:437` | The legacy launcher gives a session exactly one FigCraft port, the one reserved for its checkout, as `--forward <port>`; it exits if the connection cannot be determined |
| `launcher-common.sh:440`, `:446` | A second source: `~/.config/greywall-agents/forward/<project>` holds `<expiry epoch> <port>...`, written by the owner outside the sandbox, so a session cannot widen its own access. New sessions get the ports until the expiry. A port reserved for another checkout's FigCraft is refused (`:451`) |
| `config/bin/.local/bin/greywall-agents/figcraft.sh:12`, `:54` | The port is trusted only if the host process is FigCraft, confined to this checkout (`--control`, `--automation-read-root`, private config). Reservations are never recycled |
| `docs/security-audit-2026-10-19.md:111` | M2: the launcher once forwarded 7979 to every sandbox, so while FigCraft served project X any session could read and write X's design files. The fix is the per-checkout port above |
| `greywall-agents/claude:38` | The MCP server runs **inside** the sandbox: `figcraft-cli mcp --attach "$FIGCRAFT_CONTROL_PORT"`. Whatever connects it to the host port must exist in the namespace |
| #546 M4, M5, weakness 1 | The empty namespace closes "every sandbox reaches the host's loopback services" (the pinned engines allowed `127.0.0.1` on every port). The proxy refuses loopback unless granted by address |
| #856 comment on #829 | The launch spec can already name a Unix socket to show at a path inside the mount view, for #390 |
| #857 decision 3 | Host records: one record per project and host, rights `reach`, `credential`, `push`, never implied by each other |
| Report [14](../design/greenfield/14-plugins.md) | A plugin never widens authority: it requests, Cockpit decides |

Not checked: how `figcraft-cli mcp --attach` connects (the app is outside this repository), so
whether it honors proxy variables is unknown.

## Threats a forward adds

The adversary is #546's: the agent and the code it runs, as the owner's user.

| # | Threat | Why it needs no kernel bug |
| --- | --- | --- |
| F1 | **The service is a way out.** A forward is as strong as the service's API | A dev server with file read or exec, a Node inspector, a database, an MCP server with a shell tool, a Docker TCP port (#856 T1): each acts with the owner's rights. FigCraft reads and writes the checkout's design files (audit M2) |
| F2 | **Loopback trust.** The service sees a connection from `127.0.0.1` and may skip authentication | The relay launders the sandbox into a local client |
| F3 | **Wrong subject.** Project Y's session reaches the service of project X | Port numbers are one host-wide space (audit M2 was exactly this) |
| F4 | **Wrong target.** The port is reused after the service exits and another process answers | The grant names a number, not a program |
| F5 | **Standing access.** "Allow always" keeps working in every later session of the project, including one steered by a prompt injection | A grant is a policy, not a moment |
| F6 | **Widening by itself.** The agent asks for, or edits, its own forward | Project files and plugins may only request |

## Options

Costs are rough guesses for one agent with focused tests, as in #546.

### Q1. Supported, and carried how

| Option | Threats left | Cost |
| --- | --- | --- |
| A. Supported, as a **forward served by the launcher alongside the proxy**: for each granted service the launcher creates one more listener inside the namespace (the file-descriptor passing of #546 M4 A), at the port the agent expects, and copies bytes to the granted target, a Unix socket or `127.0.0.1:port` on the host. Counted per connection by the same subject and counters as the proxy, never parsed | F1 to F5, each answered by Q2 and Q3. The relay parses nothing, so it adds no protocol bug. The proxy keeps its loopback refusal unchanged | About 2 days on top of #829 and #831. A Unix socket target is the same code, with no port clash |
| B. Supported, **through the proxy**: `CONNECT` to a loopback address granted by address (#546 M5 already allows a grant by address) | The same F1 to F5, plus a hole in the proxy's loopback refusal, which is the check that closes weakness 1. Clients that ignore proxy variables, or that dial a loopback port (`NO_PROXY` sends it to the empty namespace), cannot use it; raw TCP, WebSocket and stdio-MCP clients are the likely ones | About 1 day, but unproven for #71's client |
| C. Not supported. A session that needs a host service uses `torlek ask run` or the owner's `agents.sandbox = "off"` | None new | Nothing to build. #71 works only in unsandboxed sessions; #856's container forwards have no path |
| D. A broker: Torlek runs the host-side client itself and the session speaks to it through the agent channel (`cockpit mcp` is already registered per launch) with per-call policy | Smaller than F1 for the tools it allows; a broker bug is a confused deputy | Weeks per protocol; serves no dev server or container port |

A and B differ in where the exception lives: in a relay that exists only for a granted service,
or in the egress proxy. Most secure: C, then D, then A, then B.

### Q2. What a grant names

| Option | Threats left | Cost |
| --- | --- | --- |
| A. A **declared service**: a record `{project, name, target, listen port, optional identity check}`, declared by a plugin or a project file and granted by the owner, who sees the target. A target is one Unix socket path or one `127.0.0.1:port`; wildcards and ranges are refused. Before each connection the relay checks the grant and, when the declaration names one, the target's identity (FigCraft: the process `figcraft.sh` verifies today). It is a record beside #857's host records, with the single right `forward`, never created by a network request | F1 stays whatever the service is. F4 closed where an identity check exists (FigCraft), open for a dev server, which has none. F6 closed: declarations request, never grant | Part of the new issue |
| B. A **bare address** the owner types (`127.0.0.1:3000`), as today's `forward/<project>` file | F4 open for every grant; the owner sees a number, not a program; one port can be a database or a debugger | Smaller than A |
| C. **Plugin-declared services only**: A, with no free-form address. A dev server needs a plugin that declares it | Same as A for plugins; nothing the owner types | A plus plugin work first (Roadmap: plugins) |

Most secure: C, then A, then B.

### Q3. Lifetime and revocation

| Option | Threats left | Cost |
| --- | --- | --- |
| A. The project grant of #546 D4, revocable, without expiry. The first connection files a request (#674's flow): allow for this session, allow always for the project, or deny. Revoking closes the open relays at once | F5 for a standing grant | Part of the new issue |
| B. Every forward expires (the legacy file's epoch), for example after 8 hours; the owner renews | F5 shortened | A timer, a renewal prompt, and a forward that vanishes mid-work |
| C. This session only: no standing grant, the owner approves again in every session | Least | One prompt per session and service; the FigCraft flow asks every time |

Most secure: C, then B, then A.

### Q4. Container published ports (the part #856 left here)

The engine of #390 runs under its own Unix user, but a rootless engine publishes ports on the
host's network, so a forward to a container is a TCP forward to a host port. The number space
is host-wide: two projects' engines can claim the same port (F3).

| Option | Threats left | Cost |
| --- | --- | --- |
| A. One service per project, **`containers`**: the target is a port the project's own engine published, asked of the engine when the connection arrives, at the address the engine publishes on. One grant, no prompt per `-p`. A port the engine does not own is refused | F3 and F4 closed by that check, which #390's tests prove (a second project's port is refused). The agent can publish any port, so the grant covers everything it publishes | Part of #390 (about half a day beyond the new issue) |
| B. Each published port is its own service and its own owner decision | The same checks, one prompt per `docker run -p` | Same, plus a prompt that interrupts compose-based work |
| C. No forwards to containers. Tests run inside the engine (`docker exec`, `compose run`); the session cannot browse a container's page | None new | Nothing. Browser-driven tests against a container fail |

Most secure: C, then B, then A.

## Recommendation

Support forwards, as **launcher-served listeners alongside the proxy** (Q1 A), for **declared
services** the owner grants per project (Q2 A), as the revocable project grant the plan
already uses (Q3 A), and give containers one service per project resolved from the engine
(Q4 A). Why: a forward needs a listener inside the namespace, which the launcher already makes
for the proxy; a relay that copies bytes cannot be confused by a protocol; and the dangerous
exception (a host service) stays out of the egress proxy, whose loopback refusal is what closes
the engines' weakness 1. The residual risk is F1: a granted service is as strong as its API,
and the owner is told what it is at the moment of the grant.

## Decisions for the owner

- [x] **Q1. Supported, and carried how.** Recommended A. Most secure: C. **Decided
  2026-11-05: A.** Forwards are supported, as a listener the launcher serves inside the
  session's namespace, alongside the proxy; the proxy's loopback refusal does not change.
- [x] **Q2. What a grant names.** Recommended A. Most secure: C. **Decided 2026-11-05: A.**
  A grant names a declared service (name, one Unix socket or one `127.0.0.1:port`, listen
  port, optional identity check), requested by a plugin or project file and granted by the
  owner per project. No wildcard, no range, no bare address.
- [x] **Q3. Lifetime and revocation.** Recommended A. Most secure: C. **Decided 2026-11-05:
  A.** A revocable project grant with no expiry. The first connection asks: allow for this
  session, allow always for the project, or deny. Revoking closes the open relays.
- [x] **Q4. Container published ports.** Recommended A. Most secure: C. **Decided
  2026-11-05: A.** One service per project, `containers`; its target is a port the project's
  own engine published; one grant, no prompt per `docker run -p`.

## Changes to the linked issues (proposed text)

Theo chose the recommended answer on all four questions, so these texts follow his decisions
as written. Nothing here is applied by this report: Theo applies them. Because a forward is
carried by the launcher and granted like a network request, #829, #831 and #674 change by a
note each; #832 is unchanged (no credential and no interception are involved). The mechanism
itself is one new issue, proposed below, whose first consumer is #390 (beta.2).

### New issue (proposed; milestone v0.1.0-beta.2, first consumer #390)

> **Title:** sandbox: owner-approved service forwards in the session launcher
>
> From [#860's report](https://github.com/Androlax2/agent-cockpit/blob/main/docs/research/2026-11-05-860-host-service-forwards.md).
> Decisions (Theo, 2026-11-05): Q1 A, a listener the launcher serves inside the session's
> namespace beside the proxy; Q2 A, a declared service granted per project; Q3 A, a revocable
> project grant with no expiry; Q4 A, one `containers` service per project.
>
> **Scope**
>
> - **Service record.** `{project, name, target, listen port, identity check?}` with one right,
>   `forward`, beside #857's host records. A target is one Unix socket path or one
>   `127.0.0.1:port`; wildcards and ranges are refused. A plugin or a project file declares a
>   service and only requests; no network request decision creates one.
> - **Launcher.** For each granted service the launcher creates the listener inside the
>   namespace (the launch spec's `forwards`, #829) and copies bytes to the target. It parses
>   nothing. It checks the grant revision and the identity check, when the record has one,
>   before each connection. A `listen` port already taken in the namespace refuses the launch
>   with the service and the reason.
> - **Not the proxy.** The proxy's loopback and private-address refusal (#831) is unchanged. A
>   forward is never an HTTP `CONNECT`.
> - **Owner decision (#674).** The first connection to an ungranted declared service files a
>   request naming the project, the service and its target: allow for this session, allow
>   always for the project, or deny. `torlek ask service NAME --reason TEXT` asks ahead, under
>   the agent's `run-commands` grant. Revoking closes the open relays.
> - **Evidence.** Per forward: connections, bytes, refusals, with the session as subject.
> - **`containers`.** The target is a port the project's own engine published (#390), asked of
>   the engine when the connection arrives and reached at the address it publishes on. A port
>   the engine does not own, including one another project's engine published, is refused.
>
> **Acceptance** (through the daemon, with a fake service on the host)
>
> - A granted `127.0.0.1:port` service answers from `127.0.0.1:<listen>` inside the session,
>   and a Unix socket target answers at its path; no other port or path in the view reaches
>   the host.
> - An ungranted service is refused at once with a reason naming `torlek ask service`; the
>   request appears in `approval list`.
> - A session of another project cannot reach the service (#71's audit M2 as a regression
>   test).
> - A target whose identity check fails is refused, naming the check.
> - Revoking the grant closes an open relay and refuses the next connection.
> - A `containers` request for a port the project's engine did not publish is refused.
> - No project file, plugin declaration or agent request adds a grant without an owner
>   decision.
>
> **Depends on:** #829, #831, #674

### #71 (covered; add to Scope and Acceptance)

> **Scope** (add)
>
> - FigCraft's control endpoint is a declared service (#860, Q2): target `127.0.0.1:<the
>   port reserved for the checkout>`, identity check the one `figcraft.sh` makes today (the
>   app confined to this checkout). The plugin requests the grant when the owner connects a
>   session; it never grants.
> - `figcraft-cli mcp --attach` runs inside the session and reaches the app through the
>   forward at the same loopback port. `FIGCRAFT_CONTROL_PORT` is set only while the grant
>   holds.
>
> **Acceptance** (add)
>
> - With `agents.sandbox = "required"`, the FigCraft MCP server connects only after the owner
>   grants the service; a session of another checkout or project cannot reach the port.
> - Revoking the grant ends the connection; the app keeps running.
>
> **Depends on:** #70, the new forwards issue (#860)

### #829 (add to Acceptance)

> - The launch spec carries `forwards`: a list of `{listen, target}`, empty until the
>   forwards issue (#860). The Unix socket #856 adds to the spec is a `target` of the same
>   field. The launcher creates each listener inside the namespace by the file-descriptor
>   passing it uses for the proxy; a `listen` port already taken refuses the launch with the
>   reason. No listener exists for an empty list.

### #831 (add a note)

> A forward to a host service (#860) is not carried by this proxy: the loopback, private-address
> and IP-literal refusals stay as written, and no grant creates an exception here. A forward's
> counters use the same subject and fields as the proxy's, so the evidence lists both.

### #674 (add a note)

> A request can name a declared service instead of a host (#860): project, service name and
> target. Choices as for a host: allow for this session, allow always for the project (a
> revocable grant, D4), deny. Revoking the grant closes the service's open relays. The feed
> event carries `kind` (host or service) and the target, so the phone (#245) can show which
> one asked. A request for `containers` (#856) keeps its own choices: allow always or deny.
> Only the owner creates a service grant; no project file, plugin or agent request does.

### #390 (replace the Forwards line)

> - **Forwards** to a container's published port use the `containers` service of #860: one
>   per project, whose target is a port this project's engine published. The engine publishes
>   on its own loopback address, and the forward refuses a port the engine did not publish
>   (a second project's engine claiming the same number is a test).
>
> **Acceptance** (add): from a sandboxed session a granted `containers` forward reaches a
> container's published port, and a port published by another project's engine is refused.
>
> **Depends on:** #829, #831, #674, #856, the new forwards issue (#860)

### #685 (add probes)

> Once the forwards issue ships: a session reaches a host loopback port it was not granted
> (by name, by IP literal, through the proxy with a host header); a granted forward is made to
> reach another target (symlinked socket, a replaced target process); a session of another
> project uses a forward. Findings are filed one per issue.

Also touched, one line each: #862 can reuse the service record if it makes a plugin a grant
subject; #832 is unchanged.

## Sources, limits and what was not checked

- Code and legacy files as cited, read at `4b45932a`. Issues #71, #829, #831, #832, #674 and
  #862 with their comments, and #390 through the #856 report, on 2026-11-05.
- Nothing was run: no relay was prototyped and no FigCraft was started. Whether
  `figcraft-cli mcp --attach` works through a listener at the same loopback port is a claim for
  the new issue's test to prove.
- Not covered: macOS (Roadmap: macOS), the owner's browser reaching a server in a session, and
  whether a plugin is a grant subject (#862).
