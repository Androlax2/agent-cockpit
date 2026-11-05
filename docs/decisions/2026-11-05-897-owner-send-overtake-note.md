# An owner's message that overtakes a held coordinator message says so, instead of waiting for it

Issue #897. Decided by the worker: the issue accepted either in-order delivery or an overtake
note, and the delivery code rules in-order delivery out at an acceptable cost.

- **The owner's `session send` still reaches the agent at once, and names what it overtook.**
  The daemon reads the coordinator messages the session's hook holds for the end of a turn
  (collected messages still `queued` while its agent works) before it delivers, appends one `[Torlek]` sentence naming them to the
  owner's text (a typed message must be one line), lists them in the reply's `held`, and
  records `overtaken_at` on each envelope still queued afterwards. The card and the
  coordinator's `agent sessions` line add "the owner's message to session … was delivered
  while it waited" to the note of a queued message that has it. Code: `crates/cockpit-daemon/src/coordination.rs`
  (`held_for`, `with_overtake_note`, `record_overtaken`), `crates/cockpit-daemon/src/sessions.rs`.
  Tests: `crates/cockpit-daemon/tests/coordinator_kit.rs` (`an_owner_message_overtaking_*`,
  `a_held_message_the_owner_overtook_*`).
- **Only a message held for the turn's end is named.** One to an agent at its prompt is
  collected within seconds, and a typed one is typed as soon as the session takes input: naming
  either would tell the agent to wait for something that may already have arrived.
- **The note never stops the owner's message.** A failure to read the held messages is logged
  and the message goes without the note. **An empty owner message is not given the note**, so it is still refused as empty; a failure
  to record `overtaken_at` after delivery is logged, not answered as a failed send, because the
  message was delivered.
- **Rejected: holding the owner's message behind the held ones.** `session send` is the owner's
  synchronous channel and answers whether the provider took the message; queuing it behind a
  collaborator's would make the owner wait on a session they outrank, for as long as the turn
  runs.
- **Rejected: handing the held messages over inside the owner's message.** It would deliver
  collaborator text through the `Addressed::Owner` path, which clears the agent's question as
  the owner's answer, and hand collected messages over mid-turn, which #703 deliberately does
  not.
