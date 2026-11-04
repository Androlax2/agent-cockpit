# A worker's question waits on the coordinator that started it before the owner

Issue #710. Decided by the owner (option A, 2026-11-04): the question leaves the owner's
needs-you list while a coordinator holds it, the card names that coordinator, and the owner
may answer it at any time.

- **The starter is a column of the session row.** `sessions.started_by` is set by both
  coordinator starts, the unconfirmed one and the one the owner approved. Routing reads the
  row it already loads; the `started` audit entry stays an audit, written after the start
  and allowed to fail without undoing it.
- **Held is derived at read time, never stored.** `question_holder` decides from the
  report's question time and escalation, the starter's state and its `message-sessions`
  grant. Nothing needs a timer: a question past its 15 minutes, of an ended or failed
  coordinator, or of one that lost the grant, reads as the owner's at the next read. The
  wire carries the outcome as `SessionReport.held_by`.
- **Only the agent's own question is held.** A dialog waits on the owner: a message would
  answer it. A question recorded before #710 has no time and waits on the owner.
- **Escalating needs no grant.** Starting the session made the question the coordinator's;
  `agent escalate` checks the sender is the starter. Asking the same question again keeps
  its time and escalation; a different question starts again with the coordinator.
- **The answer clears it on the existing path.** The coordinator's `agent message` reaches
  the worker as its next prompt, which clears the agent's question (#200).
- **Rejected:** routing by the `started` audit entry, which is written after the start and
  may be missing; a background timer that writes the escalation, which would add a writer
  for a state a read derives; a per-grant timeout (option C), not chosen.

Evidence: [routing rules with a given clock](../../crates/cockpit-sessions/tests/question_routing.rs),
[daemon and CLI fixtures](../../crates/cockpit-daemon/tests/coordinator_questions.rs) and
[the owner's card](../../crates/cockpit-tui/tests/held_questions.rs).
