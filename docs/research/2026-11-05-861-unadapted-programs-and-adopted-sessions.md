# Programs without a provider adapter, and adopted sessions

Decision issue [#861](https://github.com/Androlax2/agent-cockpit/issues/861), a decision issue
for the sandbox stage (alpha 4) with no product code. Covers
[#84](https://github.com/Androlax2/agent-cockpit/issues/84) (ACP adapter),
[#294](https://github.com/Androlax2/agent-cockpit/issues/294) (terminal templates) and
[#296](https://github.com/Androlax2/agent-cockpit/issues/296) (adopt an agent started outside
Torlek). Background: the [native sandbox report](2026-11-05-546-own-sandbox-and-traffic-control.md)
and its decisions D1 to D6, and the sibling decisions for
[#856](2026-11-05-856-docker-in-sandboxed-sessions.md),
[#857](2026-11-05-857-forge-credentials-and-push-policy.md) and
[#859](2026-11-05-859-session-folders-and-caches.md). **Research and proposal, 2026-11-05;
Theo's decisions are recorded under [Decisions](#decisions) as they are made.** Merging this
document accepts research delivery and records those answers; it authorizes no product code,
dependency or host setting, and it edits no issue. The issue texts under
[Changes for each linked issue](#changes-for-each-linked-issue) are proposals; the owner's side
applies them.

## The question

The native sandbox (#829) takes a session's hosts and folders from its provider's adapter (#830).
Two features start a session that has no such adapter, and a third creates one the sandbox never
saw:

1. **Where does an owner-configured program declare its hosts and paths?** #84 starts any ACP
   command and #294 any terminal command, "configured by the owner (command, args, env)".
   Nothing in Torlek knows what they need.
2. **What does a program that declares nothing get?**
3. **May an adopted process exist under the default-on rule, and how do its card and evidence
   show it?** #296 shows an agent the owner started by hand. It was never launched, so it was
   never sandboxed, while #833 makes the sandbox required by default.

## Recommendation and decisions

1. **Declare in the owner's config entry** for the program: `hosts`, `read`, `write`. The file is
   host-side, which a session cannot read or write, so the declaration is the owner's own
   statement. A project file may ask for more; like `[needs]` it requests and never grants.
2. **A program that declares nothing gets the floor and no network**: its checkout, session
   folder, temp folder and its own executable, nothing else of the home, no host. Every host is
   an owner decision (#674), worded by Torlek.
3. **Adoption is allowed and Torlek gives it no authority.** The process is already running with the
   owner's rights and refusing it makes it invisible, not safer. Adopting adds a card that says
   `UNSANDBOXED, adopted`, takes messages from the owner only (never from another session), has
   no agent-channel identity and can be given no grant.

- [x] Q1 decided 2026-11-05: A (see [Decisions](#decisions)).
- [x] Q2 decided 2026-11-05: A (see [Decisions](#decisions)).
- [x] Q3 decided 2026-11-05: B (see [Decisions](#decisions)).

## What exists today

Code at this branch's base (`e1132851`); paths are relative to the repository root.

| Where | Fact |
| --- | --- |
| `crates/cockpit-sandbox/src/launch.rs:188` | The daemon passes `allow_hosts: Vec::new()`. Today every host and provider folder comes from Greywall's reviewed profile for the provider (`--auto-profile --profile <provider>`, `greywall.rs:335`) |
| `crates/cockpit-sandbox/src/greywall.rs:25`, `:289` | `ENGINE_PROFILES = ["claude", "codex", "opencode"]`; any other provider is refused ("greywall has no reviewed profile for it"). Today an unadapted program can only run with `agents.sandbox = "off"` |
| `crates/cockpit-provider-api/src/lib.rs:192`, `:215` | `Provider::launch` returns a `LaunchCommand` (program, arguments, environment). A provider has no way to state what hosts or folders it needs; #830 adds that for the three adapters |
| `crates/cockpit-sandbox/src/launch.rs:16`, `:21` | `HostPaths.denied` holds Torlek's own files "no agent reads": the host-only socket, the state database, logs, the owner's configuration |
| `crates/cockpit-sandbox/src/profile.rs:17`, `:36` | `BASELINE_DENIED_READ` (`~/.ssh/id_*`, `~/.aws/**`, `.env`...) and `BASELINE_DENIED_WRITE` (`.git/hooks`, `.git/config`, `.cockpit`, `.torlek`, `~/.local/bin`...) apply to every profile |
| `crates/cockpit-config/src/lib.rs:230-238` | `AgentSandbox::{Required, Off}`; `Off` is "the one named opt-out", set by the owner for the whole profile |
| `crates/cockpit-sandbox/src/evidence.rs:55` and `crates/cockpit-protocol/src/sessions.rs:260` | `Enforcement::Unsandboxed { reason }`, documented "only ever by the owner's explicit setting". The card and `session show` print `sandbox: NONE on <platform>, unsandboxed: <reason>` (`sessions.rs:283`) |
| `crates/cockpit-config/src/lib.rs:227`, `crates/cockpit-daemon/src/instance.rs:47` | `UNSANDBOXED_RISK`: an unsandboxed agent can reach the host-only owner socket and act as the owner, so Torlek's grants hold only under the sandbox (#729, closed: it stopped accidents, not the reach) |
| `crates/cockpit-authorization/src/model.rs:57` | `MessageSessions`: "queue a message to another session, delivered at its next prompt (#410)" |
| #84, #294, #296 (issue text) | #84 and #294 read the program from the owner's configuration; #294's templates live in `templates/<name>.md` beside it and roles for teams (#103) reuse the file shape. #296 adoption "creates a session with execution identity from the live process, no launch token ... no run-commands grant until the owner gives one", refused when the process is not the owner's or not in the private tmux server |
| #546 report, "Torlek-native design" 4 | "Project files can declare hosts they need; like `[needs]`, they request and never grant" |
| #859 (comment on #829) | Only the toolchain roots named by #830's table are bound; "a `PATH` entry outside the table, such as `~/.local/bin`, is not mounted" |

## Threats

The adversary is the one in the #546 threat model: the program and the content it reads (prompt
injection from issues, pages, dependencies), working as the owner's OS user. Unadapted programs
add that Torlek knows nothing about the program's behaviour or its hosts.

1. **No declared limit becomes a broad one.** A fallback of "any public host" makes every
   unadapted program an exfiltration channel to anywhere.
2. **The declaration is written by whoever the program obeys.** A project file arrives with a
   cloned repository; a template can be copied from the web. A source that grants silently lets
   an untrusted repository widen the network.
3. **Self-widening paths.** A declared path in `BASELINE_DENIED_READ` or `BASELINE_DENIED_WRITE`,
   the owner's configuration, state or a socket folder would undo the floor.
4. **Owner fatigue.** Each undeclared host is a request. A flood of them trains approval; request
   text written by the program would let it phrase its own approval.
5. **The program's own credentials.** A program signs in somewhere (`~/.config/<program>`). That
   folder is declared `write` and read only by that program; it must not be a path another
   profile can read.
6. **An adopted process is already outside.** It runs with the owner's rights and reaches the
   owner socket (#729). What Torlek can add: authority (a grant that reads as a bound but is not),
   an inbound channel (a sandboxed session with `message-sessions` queues text into a process
   that can do anything: a confused deputy that turns a prompt injection in the sandbox into an
   unsandboxed action), or a false picture (a card that looks like the sandboxed ones).
7. **Adopting the wrong process.** Not the owner's, or outside the private tmux server: #296
   already refuses both.

## Options

### Q1. Where a program declares hosts and paths

| Option | Threats it leaves | Cost |
| --- | --- | --- |
| **A. The owner's config entry for the program** (recommended). `hosts`, `read`, `write` in the entry (#84) or the template header (#294); a session cannot read or write it. Evidence names the source ("declared by owner config"). A project file may request more; the request goes through #674 and grants nothing | T2 only if the owner copies a hostile template without reading it: Torlek shows the declared hosts and paths when the program is added and in `torlek sandbox show`. T5 is the declared `write` folder, by design | One short table per program. The declaration is a source like an adapter's, not a stored grant: editing the entry changes the next launch, deleting it revokes |
| **B. The project file, as a request.** Every project asks the owner for the program's hosts at its first launch | T2 closed. T4 worse: every project asks again for every program | More owner decisions for a thing the owner already wrote once in the program's own entry |
| **C. Both, the project file granting.** Union of the two | T2 open: a cloned repository's file widens the network with no decision | Least friction. Contradicts the #546 rule that project files request and never grant |

Rules that apply to any option, from the code above: `hosts` are exact names with an optional
port, or `*.domain`; a bare `*`, a wildcard on a public suffix (`*.com`, `*.co.uk`, `*.github.io`;
the suffix list is the implementing issue's choice, a dependency this report does not make), an
IP literal and a private or metadata range are refused when the entry loads (the proxy refuses
the last two anyway, #831). A `read` or `write` path inside
`BASELINE_DENIED_READ` / `BASELINE_DENIED_WRITE`, the owner's configuration, the state folder or
a container socket is refused when the entry loads, the rule #859 sets for a project's extra
folder. The program's executable is bound read-only **by itself** (its resolved file), not its
folder: binding `~/.local/bin` is what #859 refuses. Its libraries and install tree must be named
in `read`. A folder one entry declares `write` (its sign-in state, T5) is refused in every other
entry's `read` or `write` and in any project request, checked when entries load.

### Q2. What a program that declares nothing gets

| Option | Threats it leaves | Cost |
| --- | --- | --- |
| **A. The floor, no host** (recommended). Checkout, session folder, temp folder, the executable, the environment Torlek builds, no host and no other path. The registry hosts of #859 are treated like a provider's hosts, so a program that declares none gets none; it declares them in `hosts` or the owner decides the request. Every host is a #674 request that names the program as "declares no hosts" | T4: the first launch asks for the program's model API and perhaps a few more | A program that needs the network stalls on its first request until the owner answers; the 403 body says so at once (#831) |
| **B. Refuse the launch** until the entry has a `hosts` key (an empty list says "needs none") | None beyond A's | A forced edit of the config for a program that needs no network or only a local model. Same outcome as A for owners who answer requests |
| **C. Any public host on 443**, private ranges refused | T1: every unadapted program can send data to any host | No friction. The only option that adds a path out by default |

### Q3. An adopted, unsandboxed process

| Option | Threats it leaves | Cost |
| --- | --- | --- |
| **A. Refuse adoption unless `agents.sandbox = "off"`** | None that Torlek adds: no card, no channel, no messages. The process exists either way | The owner's hand-started agent is invisible to Torlek; #296 does not work by default. The owner learns of the process from nothing |
| **B. Adopt it as an observed session with no authority** (recommended). Allowed under `required`. Evidence `Unsandboxed { reason: "adopted: started outside Torlek, never sandboxed" }`; a badge on the card; no launch token and no agent-channel identity, so it holds no grant and none can be given (a grant would read as a bound, #729); the owner's messages reach it, a session's never (`message-sessions` to it is refused with the reason); the audit log records who adopted it, when and through which client (#672); an `ask` or an owner verb from it is not accepted, it has no identity | T6: it keeps its owner rights, which Torlek does not change. The owner's own messages can still instruct it | An adopted agent cannot take part in a team (#103) or be addressed by a coordinator. One rule and one evidence variant to keep |
| **C. #296 as written**: adopt, and the owner may give it `run-commands` | T6 all: a grant that does not bind (#729), a card that looks like the others, messages from sessions | None |

If the owner wants the process inside the sandbox, the way is a new sandboxed session, not a
promotion: namespaces, Landlock and seccomp cannot be applied to a running process. For a
provider that resumes, the card's next action is `torlek session new --resume`; this report does
not build it.

## Decisions

Theo's answers, asked one at a time in his session.

- [x] **Q1.** Decided 2026-11-05: **A**. Only the owner's config entry for the program, on the host, declares its hosts and paths and so grants them. A project file may only request, through #674; it never grants.
- [x] **Q2.** Decided 2026-11-05: **A**. A program that declares nothing gets the floor and no host; every host is a #674 request that names the program as "declares no hosts".
- [x] **Q3.** Decided 2026-11-05: **B**. An adopted process is allowed under `required` as an observed card with an `UNSANDBOXED, adopted` badge. It has no agent-channel identity and no grant, and none can be given; only the owner's messages reach it, never a session's.

## Changes for each linked issue

Proposed text; the owner applies it. Nothing here edits an issue. Common to all: one `Needs`
type (`hosts`, `read`, `write`) in `cockpit-provider-api`, produced by an adapter (#830) or by
the owner's config entry for a program, and consumed by one enforcement path (#829, #831).

### #84 cockpit-acp: generic adapter for any ACP CLI

Add under Scope:

> **Sandbox needs (#861, Q1 and Q2).** An ACP command is an unadapted program. Its config entry
> (command, args, env) also declares `hosts` (exact name with optional port, or `*.domain`),
> `read` and `write` (paths). Only this host-side entry grants them; a project file may request
> more through #674 and never grants. An entry that declares nothing gets the floor: the
> checkout, session folder, temp folder, the executable's resolved file (not its folder), and no host or other path
> (registry hosts are declared in `hosts` like any other, or decided as requests). A bare `*`, an IP literal, a private or
> metadata range, and a path in `BASELINE_DENIED_READ`/`BASELINE_DENIED_WRITE`, the owner's
> configuration or state, or a container socket is refused when the entry loads. Baseline only
> stays baseline: this issue adds no ACP client method that would move file or process work out of
> the sandbox.

Add to Acceptance: "With `agents.sandbox = "required"` and a fixture agent whose entry declares
nothing, the session starts with the floor only, and a request to any host gets `403` naming the
program as declaring no hosts (#831, #674). With `hosts = ["fixture.invalid:8443"]` that host
connects and no other does."

Depends on: add #674, #829, #830, #831, #861.

### #294 Generic terminal adapter from a template

Add under Scope:

> **Sandbox needs (#861, Q1 and Q2).** The template header gains `hosts`, `read` and `write`,
> with the rules and the floor of #84's text. Templates live under the owner's configuration,
> which no session can read or write, so the header is the owner's declaration. A role for teams
> (#103) reuses the header; its grants are separate and never widen `hosts`. The executable is
> bound by its resolved file only; its install tree must be named in `read`.

Add to Acceptance: "A template with no `hosts` runs under `agents.sandbox = "required"` with no
reachable host; with `hosts` set only those connect; a `read` path inside `BASELINE_DENIED_READ`
is refused when the template loads."

### #296 Adopt an agent started outside the cockpit

Replace the second Scope bullet and the Acceptance with:

> - **Allowed under `agents.sandbox = "required"` as an observed card, with no authority (#861,
>   Q3).** Adoption is an owner verb, answered `owner_only` on the agent socket. It creates a
>   session with execution identity from the live process (report 04), no launch token, no
>   agent-channel identity and no grant; no grant can be given to it, because an unsandboxed
>   process reaches the owner socket (#729) and a grant would read as a bound it is not. Torlek
>   gives it no authority; it does not remove what the process already has, and the card and
>   `session show` say so ("can reach the owner socket"). It
>   reports (state from the hook bridge or OSC 7501), sleeps and ends like any session.
> - **Messages:** the owner's reach it. A message from another session, including with
>   `message-sessions`, is refused with the reason ("this session is unsandboxed and adopted;
>   only the owner messages it"), so a prompt injection in the sandbox cannot turn it into an
>   unsandboxed action. It is not a valid target for `start-sessions` or a team role.
> - **Shown as what it is:** evidence `Unsandboxed { reason: "adopted: started outside Torlek,
>   never sandboxed" }`, the card badge `UNSANDBOXED, adopted`, `session show` printing
>   `sandbox: NONE ... adopted`, and the audit log recording who adopted it, when and through
>   which client (#672). Refused when the process is not the owner's or not inside the private
>   tmux server. It is not sandboxed after the fact; the card's next action is a new sandboxed
>   session.
>
> **Acceptance:** an agent started by hand appears on a card with its state, the badge and the
> evidence line, and the owner can message it; with `required` set, a message from a sandboxed
> session with `message-sessions` is refused; a grant request for it is refused; the audit log
> shows the adoption; the card and `session show` state that it can reach the owner socket; a process of another user or outside the private tmux server is refused.
> **Depends on:** add #672, #829, #833, #861.

### #829 native Linux sandbox launcher

Add to Acceptance:

> - The launch spec carries the program's `Needs` and its source, either the adapter or the owner's
>   config entry (#861). The evidence names the source; a program with none gets the floor. A
>   declared path in `BASELINE_DENIED_READ`/`BASELINE_DENIED_WRITE` is refused at load, as is a
>   folder another entry declares `write` (a program's sign-in state, readable by it alone), and a
>   declared executable is bound as its resolved file.
> - `Enforcement::Unsandboxed` is no longer "only by the owner's explicit setting" (its doc
>   comments, `evidence.rs` and `cockpit-protocol/src/sessions.rs`): a second reason is an
>   adoption by the owner (#296). A launch that cannot be enforced is still refused under
>   `required`.

### #830 provider hosts and paths declared in the provider adapters

Add:

> The declared shape is one `Needs` type that adapters and the owner's config entries for
> unadapted programs (#84, #294) both produce. Greywall's refusal of a provider with no reviewed
> profile (`greywall.rs:25`, `:289`) leaves with the engines; a program without an adapter is
> launched only through an owner entry, and one that declares nothing gets the floor and no host
> (#861). A project file's hosts are requests (#674), never part of `Needs`.

### #831 per-session egress proxy

Add:

> Allowlist sources are the adapter's or the owner entry's `hosts`, the detected ecosystem's
> registry hosts (#859) for a session whose provider declares them, and project grants (#674). Evidence and counters name the source of each entry.
> The `403` for a program that declares no hosts says so ("`<program>` declares no hosts") so the
> owner knows to declare them or to decide the request.

### #674 refuse at once, owner decides

Add:

> A request from an unadapted program carries its name and whether its entry declared hosts; the
> text shown to the owner is built by Torlek from the `CONNECT` host and port, never from text the
> program supplies. A project file's declared hosts arrive here as pre-filled requests and grant
> nothing until decided (#861, Q1). An adopted process (#296) makes no request: it never goes
> through the proxy.

### #833 turn the native sandbox on by default

Add to Acceptance:

> - Adopting under `required` is allowed as #296 describes (#861, Q3); `torlek doctor` and the
>   session list count adopted unsandboxed sessions, so "the sandbox is on" is never read as
>   covering them. A launch that cannot be enforced is refused as before; no adoption path starts
>   a program unsandboxed.

### Not changed

Issue #832 (a program's own sign-in folder, declared `write` in its entry, is a folder grant
that only that program reads, not a credential bound to a proxy host; a host credential still
needs #832's explicit binding), issue #103 (roles keep their grants separate from declarations), issue #293 (OSC 7501
stays display-only, never trusted for authorization), and issues #156 and #155 (unaffected). `agents.sandbox = "off"` stays the
one named opt-out for launches: there is no per-program bypass.
