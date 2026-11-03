# A Claude Code session collects messages from other sessions through an asyncRewake hook, never its input box

Issue #703. Decided by the coordinator session sf27f63e06fde (option A), not by Theo: Theo's
requirement in #703 is only that messages from other sessions never go through the input box
as if the owner wrote them. Measured on Claude Code 2.1.317 (the hook option is also in 2.1.314
and 2.1.316).

- **The bridge waits for messages in a background hook.** With the bridge, launches add a
  second hook on `SessionStart` and `Stop`: `torlek-claude-hook <torlek> --inbox`, with
  `asyncRewake: true` and a 24-hour timeout (`hooks::INBOX_EVENTS`). Claude Code runs it in
  the background; it runs `torlek agent inbox`, which asks the agent channel (`agent-inbox`)
  every 2 seconds. A collected message goes to standard error with exit status 2, which
  wakes the model with the text as a system reminder ("Stop hook feedback"); nothing is
  typed in the terminal. A failure exits 1, never 2, so an error never reaches the model as
  a message. The adapter reports this as `Capability::Inbox`; the daemon routes on the
  capability, never on the provider's name.
- **Messages wait for the turn's end.** The daemon hands nothing over while the agent's last
  report is `working` or it has a dialog open; the next `Stop` hook collects it. Whether
  asyncRewake wakes a model in the middle of a turn was not measured, so it is not relied on.
- **One waiter per session.** Each `Stop` starts a new hook, so the newest waiter takes over
  and an older one ends quietly (`superseded`). The daemon keeps the current waiter in memory
  (`coordination::Waiters`). A waiter whose connection drops, such as at a daemon restart,
  connects again; the first waiter that asks the new daemon is adopted.
- **Known limits.** A session launched before this change has no inbox hook: its messages
  show `stuck` until it is restarted. Claude Code ends a hook after its timeout (24 hours,
  as long as a message lives), so an agent idle longer has no waiter until its next turn.
- **`received` means handed to the hook.** A collected message is claimed from the outbox and
  recorded `received` in the same transaction that answers the hook, under the producer
  `coordinator-inbox`, so the typing loop never claims it. The sender's grant is checked at
  collection, as before typing. A session without the capability (Claude Code without the
  bridge, other providers) keeps the typing loop.
- **The sender sees why a message waits.** `torlek agent message` says `held: …` with the
  reason, and `torlek agent sessions` shows the sender its own message to each session, its
  delivery state and a note: no report yet, a dialog open, held until the turn ends, or
  `stuck` when the agent has waited at its prompt for 60 seconds without its hook collecting
  it (its Claude Code may not run asyncRewake hooks, or it started before this change).
- **Rejected: an MCP channel** (`notifications/claude/channel`). It needs
  `--dangerously-load-development-channels`, which shows a warning dialog at every launch
  (the stuck-at-start problem of #702), a claude.ai sign-in and the organization policy
  `channelsEnabled`.
- **Rejected: typing better** (waiting for the prompt, or queuing with Ctrl-X Enter as
  before). The owner ruled out the input box; on 2.1.317 the queued text also stayed unsent
  while the outbox recorded it `received`, the bug #703 reports.

Code: `crates/cockpit-claude/src/bin/cockpit-claude-hook.rs`, `crates/cockpit-daemon/src/coordination.rs`
(`collect`, `collection_note`). Tests: `crates/cockpit-claude/tests/inbox_bridge.rs`,
`crates/cockpit-daemon/tests/coordinator_kit.rs`.
