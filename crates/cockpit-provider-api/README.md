# cockpit-provider-api

The contract every agent provider adapter implements: its capabilities and how to launch, or resume, one of its conversations.

A provider is an agent program (Claude Code, Codex, OpenCode) seen through its adapter.
`cockpit-sessions` owns the session and its lifecycle; it asks the provider only for the
command that starts or resumes a conversation, and what the provider can do. Nothing above
this contract is keyed on a provider's name, only on its reported [`Capabilities`].

This is the **baseline** contract issue #31 needs: launch (new or resumed conversation, an
optional first message) and the `resume` capability. Issue #33 extends it (capability
provenance, submit, events, cancel, outcome, commands, evidence) and adds the Claude adapter;
no Claude type or path belongs here or in sessions.

## Rules

- A provider names the conversation it started when it can ([`LaunchCommand::conversation`]);
  a session whose conversation is unknown cannot be resumed, restarted or put to sleep.
- A resumed launch keeps the conversation it was asked for.
- [`ProviderName`] and [`ConversationId`] are validated where they are made; the command's
  words are never parsed by a shell.

## Dependency edges

- Depends on: the standard library only (`tests/dependencies.rs`).
- Must not depend on a user interface (`providers -> UI` in the crate-graph rule).
- Used by: `cockpit-sessions`, the daemon's wiring, and the adapters (#33).

## Tests

- `tests/names.rs`: provider names and conversation ids.
- `tests/dependencies.rs`: no dependency.
- The fake provider that implements this contract lives in
  `cockpit-daemon/tests/sessions.rs`.
