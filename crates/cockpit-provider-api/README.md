# cockpit-provider-api

The contract every agent provider adapter implements: identity, sign-in and capabilities with provenance, launch and resume, terminal and delivered actions, native commands and normalized events.

A provider is an agent program (Claude Code, Codex, OpenCode) seen through its adapter.
`cockpit-sessions` owns the session and its lifecycle; it asks the provider for the command
that starts or resumes a conversation, the keystrokes of an action, and what it can do.
Nothing above this contract is keyed on a provider's name, only on its reported
[`Capabilities`]. No adapter type or path belongs here or above it (#33).

## Rules

- **Sign-in.** [`Provider::sign_in`] reports [`SignInStatus`] with independent provenance:
  signed in, not signed in with the harness's next step, or unknown with a reason and next
  step. This is the harness's local credential status, not proof of model access, a valid
  token or host authorization. Probes are bounded, run off async workers, and never return
  raw credential or account output. The default is unknown; adapters own detection.

- **Capabilities with provenance** (report 08). Each reported [`Capability`] is `tested`
  against the exact harness version running (a version on the adapter's
  `tested-versions.txt`, #732), `probed` (not tested, but the runtime probe found every flag,
  name and event the adapter relies on), or `discovered` (shown as unverified); one not
  reported is unavailable. A probe that misses a requirement takes away the capabilities
  that need it and names it ([`Probe::summary`]). Resume, submit, events, cancel, outcome, effort, model, compact,
  remote control, commands and screen reading have contract behavior; inbox says the
  adapter's bridge collects messages from other sessions itself (#703); evidence and billing are
  named so a capability report can show them, and get their methods with their first caller.
- **Launch.** A launch refuses a request that needs a capability the provider does not
  report ([`LaunchRequest::needed_capabilities`]): a resumed conversation, an effort level, a
  model, a compaction threshold, remote control. The adapter validates an effort, a model or
  a compaction threshold in its harness's own words; a launch that only builds words checks
  them before anything is recorded (#701, #783). A capability the installed harness version
  lacks is withheld with the missing requirement as the reason
  ([`Capabilities::withheld`]), and the refusal repeats it. A
  provider names the conversation it started when it can
  ([`LaunchCommand::conversation`]); a session whose conversation is unknown cannot be
  resumed, restarted or put to sleep. A resumed launch keeps the conversation it was asked
  for. Each launch gets the session's private directory ([`LaunchRequest::session_dir`]),
  made before the launch, reachable by the agent and the daemon (granted inside the sandbox,
  #437), and short enough to hold a unix socket.
- **Actions.** A provider performs submit, cancel and compact one of two ways. Terminal
  actions ([`Provider::input`]) become text and keys for the agent's terminal, decided from
  its current [`Activity`]; the provider refuses one the activity makes unsafe, such as
  typing into an open dialog. Delivered actions ([`Provider::deliver`]) are performed by the
  adapter itself in a named conversation, through what the launch left in the session's
  directory, and say whether they started a turn, joined the one under way or interrupted it
  ([`Delivered`]; Codex, #407). The agent writes that directory, so the adapter opens a
  socket there only through the caller's [`SessionSockets`] ([`DeliveryTarget`]), which
  refuses one whose listener is not the session's own process (#442). Only the typed
  `Delivered::Unsupported` result permits terminal fallback; an error never does (#431).
  Both refuse an action
  whose capability is not reported.
- **Native endpoints** (#416). [`DeliveryTarget::launch`] carries the command the host retained
  for this launch, never data read back from the agent. OpenCode reads its server's loopback
  port and password from it. Missing metadata refuses delivery; it is not permission to type
  into a terminal. [`Delivered::Accepted`] records an accepted or queued message without
  claiming a turn started. The daemon's calls and launch retention belong to #431.
- **Native commands.** [`Provider::commands`] returns names, descriptions and the harness's
  source labels; [`Provider::invoke_command`] runs one in a named conversation. Both require
  `commands` and default to unsupported. The capability carries tested/discovered provenance.
  Commands are explicit operations, never inferred from submitted text; a reply is not proof
  of task completion. A timed-out invocation may already have been accepted by the harness.
- **Events.** Adapters map their harness's events onto [`EventKind`] and keep the raw event
  beside it; unknown events stay raw. Acceptance of a prompt is not completion: completion
  arrives as a turn-end event.
- **Screen reading** ([`Provider::read_screen`]) is a fallback for missing events and records
  its confidence (`observed-by: event|screen`, report 05); it never proves a task complete.
- **Sandbox needs** (#830, #861). [`Provider::needs`] declares the [`Needs`] of its harness:
  the [`Host`]s it reaches, the [`HomePath`]s it reads from the owner's home (bound
  read-only: its installed program, never a credential) and those it writes, which the
  sandbox backs with a folder Torlek owns for the session, kept across its relaunches and
  never the owner's (#858, #859). [`Provider::credentials`] names where the harness stores a
  sign-in and which variable takes a bound credential on which of its hosts (#858); every
  credential host is a declared host, and no read path covers a credential file. The default
  is the floor: no host and no folder beyond the checkout and the session's own. The same
  `Needs` is what the owner's entry for an unadapted program produces (#84, #294); a project
  file's hosts are requests the owner decides (#674), never part of it. A malformed value is
  refused, never narrowed: no bare `*`, no wildcard on one label, no single-label or
  `localhost` name, no IP address in any form a resolver reads, no path outside `~/`, and a
  path wildcard only as one trailing `*` after a prefix that is not only dots. The declarations request; the sandbox (#829, #831, #832) enforces and refuses
  what its own rules forbid.
- **Toolchain table** (`toolchains`, #859). For Rust, Node, Python and Go: the marker files
  that detect the ecosystem in a checkout, the install roots bound read-only where the launch's
  `PATH` names a folder inside them, the variables that keep caches and configuration in the
  session's folders, and the registry hosts. It holds no write access to the owner's home and
  no credential.
- **Attribution.** The adapters' declarations and the toolchain table are seeded from
  Greywall's profiles at its pinned commit, under the Apache License 2.0; `NOTICE-greywall`
  carries its notice and what Torlek changed.
- [`ProviderName`] and [`ConversationId`] are validated where they are made; the command's
  words are never parsed by a shell.

## The fake provider

Feature `fake` ships `fake::FakeProvider` for other crates' tests: the baseline contract
only (launch a new or resumed conversation, `resume`), a shell agent that logs each launch
and marks the session's directory.
`cockpit-daemon`'s session tests launch it; release builds never enable the feature.

## Dependency edges

- Depends on: the standard library only (`tests/dependencies.rs`).
- Must not depend on a user interface (`providers -> UI` in the crate-graph rule).
- Used by: `cockpit-sessions`, the daemon's wiring, and the adapters (`cockpit-claude`,
  `cockpit-codex`).

## Tests

- `tests/names.rs`: provider names and conversation ids.
- `tests/capabilities.rs`: provenance, availability and refusals.
- `tests/tested_versions.rs`, `tests/probe.rs`: the tested list, the probe's findings and its helpers.
- `tests/contract.rs`: the capabilities a launch needs; default refusals, delivery included.
- `tests/events.rs`: the activity each normalized event proves.
- `tests/fake.rs`: the fake provider and its agent's log.
- `tests/needs.rs`: hosts, home paths, needs and credentials, and the floor by default.
- `tests/toolchains.rs`: the toolchain table, detection and the roots a `PATH` binds.
- `tests/dependencies.rs`: no dependency.
