# cockpit-provider-api

The contract every agent provider adapter implements: identity, sign-in and capabilities with provenance, launch and resume, terminal actions and normalized events.

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
  against the exact harness version running, or `discovered` (shown as unverified); one not
  reported is unavailable. Resume, submit, events, cancel, outcome, effort, compact, remote
  control and screen reading have contract behavior; commands, evidence and billing are
  named so a capability report can show them, and get their methods with their first caller.
- **Launch.** A launch refuses a request that needs a capability the provider does not
  report ([`LaunchRequest::needed_capabilities`]): a resumed conversation, an effort level,
  remote control. A provider names the conversation it started when it can
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
  refuses one whose listener is not the session's own process (#442). Both refuse an action
  whose capability is not reported.
- **Events.** Adapters map their harness's events onto [`EventKind`] and keep the raw event
  beside it; unknown events stay raw. Acceptance of a prompt is not completion: completion
  arrives as a turn-end event.
- **Screen reading** ([`Provider::read_screen`]) is a fallback for missing events and records
  its confidence (`observed-by: event|screen`, report 05); it never proves a task complete.
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
- `tests/contract.rs`: the capabilities a launch needs; default refusals, delivery included.
- `tests/events.rs`: the activity each normalized event proves.
- `tests/fake.rs`: the fake provider and its agent's log.
- `tests/dependencies.rs`: no dependency.
