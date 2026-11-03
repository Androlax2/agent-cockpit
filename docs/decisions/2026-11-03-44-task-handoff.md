# Explicit task handoff uses scheduling's placement transaction

Issue #44. Decided by the worker within Theo's basic task-view assignment, to reuse
existing placement authority and durability without changing the schema.

- **An additive owner-only request.** `TaskHandoff` names the item, its displayed
  intent and state revisions, the selected session and its launch number. Existing
  `TaskStart`, queue and edit requests retain their wire contracts. The CLI requires
  the three snapshot flags (`todo show` and `todo targets` expose their values); the TUI captures them when its picker opens. Neither
  refreshes refused intent into a new request automatically.
- **One current-state check and placement transaction.** The daemon reads the selected
  session's launch and current report in the same write transaction as the task
  revision comparison, scheduling eligibility, admission reservation, placement and
  outbox insertion. The selected item may be unqueued: selection is the owner's
  explicit opt-in. All remaining eligibility checks stay in force, including project,
  capabilities, existing work, viewed-session exclusion, the retained idle window and queue pause.
- **Existing delivery and reply.** The ordinary durable outbox delivers the placement;
  retries and uncertainty retain their existing semantics. The response is the current
  `Task` view after the delivery attempt, without a claim about unrelated waiting work.
  A running workflow item is refused with direction to its workflow view: a task
  revision does not identify a workflow step or attempt.
- **Plain view, retained drafts.** `T` opens tasks, `a` adds, `e` edits, `h` picks a
  session, `Q` sets the queue flag, and `?` shows the last queue explanation. Editing
  retains the original intent revision and text on refusal. The existing queue command
  remains an explicit boolean set. Task trail, updates, reviewed/return notes and defer
  views remain excluded.
- **Rejected:** direct item/session SQL in the TUI, impersonating an agent through
  `TaskStart`, or typing an untracked message as placement. Each would omit existing
  transaction or admission ownership.

Evidence: [daemon socket fixtures](../../crates/cockpit-daemon/tests/scheduling/handoff.rs),
[App and rendering fixtures](../../crates/cockpit-tui/tests/tasks.rs),
[terminal fixture](../../crates/cockpit-tui/tests/e2e/tasks.rs),
[CLI process fixtures](../../crates/cockpit-cli/tests/queue/handoff.rs) and
[wire compatibility](../../crates/cockpit-protocol/tests/task_handoff.rs).
