# An agent fixes the title and notes of the task items it filed itself

Issue #714 (v0.1.0-alpha.3). Decided by the worker, within the issue's Expected; the open
questions the issue left are answered below.

- **The rule lives in `cockpit-tasks`, not the socket.** `routes.rs` sends `task-edit` from the
  agent socket to `tasks::route` like `task-add` and `task-delete`; `cockpit_tasks::edit`
  decides. A session edits an item only when its `filed_by` is the session, nobody started it
  (`proposed` or `todo`, the statuses `delete` already allows a session), and the edit names
  only a title or notes. Priority, queue, approve, decline, reopen, waits, images and drafts
  keep answering "the owner's alone" on the agent socket; `routes.rs` never sends them to
  `cockpit-tasks` for an agent.
- **"Filed itself" is `filed_by`**, the session shown by `torlek todo show`. A session keeps
  its id across restarts and resumes, so it keeps editing what it filed; a replacement session
  with a new id does not. An item in another project the session did not file is not visible
  to it and answers "no item", as every other verb does.
- **`--kind` is not editable by a session.** The issue's Expected names the title and notes;
  the kind is how the owner sorts work, so a change of it answers "only the owner can change
  an item's kind". An edit that names no title and no notes is refused as invalid instead of
  bumping the intent revision for nothing.
- **A proposal is edited like a `todo`.** Both are the session's own words that nobody acted
  on yet: the owner decides a proposal on its current text, as they would read a filed item
  before starting it. A started, done or declined item answers "item #N is doing; it cannot be
  edited by the session that filed it", naming the status.
- **Every session edit is audited** in `authorization_audit` as `edited`
  (`AuditedAction::Edited`), under the session's `change-state` grant and the revision the
  decision read: actor the session, no target, detail `item N: title "old" -> "new"; notes
  "old" -> "new"` naming only the fields that changed. A value longer than a title (200
  characters) is kept as its head and its length: audit rows are never removed and `session
  grants` ships them whole, so two full copies of 4000-character notes per edit would not do.
  `record_audit` now takes `change-state` beside the coordinator grants
  (`Permission::is_audited`); migration 6 of `cockpit-authorization` rebuilds the audit table
  for the new action, as #657 and #713 did. The owner reads it with `torlek session grants
  SESSION`. **Known limit:** that listing shows the newest 20 rows of every action, so a
  session's many edits push its coordinator entries out of the 20. The owner's own edits are
  not audited: they are the owner's. A refused edit is not audited.
- **An edit that changes nothing is no edit**, for the owner and a session alike: the intent
  revision stays and nothing is audited, so an agent's retry after a timeout does not move
  the revision the owner's form holds, and the audit records no `"x" -> "x"`.
- **`task-edit` carries a launch token on the wire**, like every other task verb a session
  sends, so an edit that is a launch's first request binds it instead of answering
  "unidentified peer". The owner sends none, as before.

Evidence: `crates/cockpit-daemon/tests/tasks.rs`, `crates/cockpit-tasks/tests/items.rs`,
`crates/cockpit-authorization/tests/coordinator.rs`.
