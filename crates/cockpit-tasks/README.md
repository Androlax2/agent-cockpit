# cockpit-tasks

Owns the requested outcome of work: items with their kind, priority and status, the owner's
decisions on proposals and what was declined, waits on other items and the summaries they
pass on, filings across projects under a cap, blocker reports, note images, drafts, and the
split between intent and state revisions. Design: [report 06](../../docs/design/greenfield/06-tasks-workflows-verification.md).

It does not sequence steps (`cockpit-workflows`) or place work on sessions
(`cockpit-scheduling`): `queued` says an item is available, `next_action` says what has to
happen for it to move.

## Items

| Status | Means | Leaves by |
| --- | --- | --- |
| `proposed` | a session suggested it | the owner's `approve` (`todo`) or `decline` |
| `todo` | wanted | `start` (`doing`) or the owner's `finish`, both refused while it waits on unfinished items |
| `doing` | a session (or the owner) works on it | `finish` (`done`, with a summary) or the owner's `reopen` |
| `done` | finished; its summary is what the items waiting on it read | the owner's `reopen` |
| `declined` | the owner said no | never: the title stays declined in its project |

Kinds: `task`, `feature`, `bug`, `improvement`, `chore`, `research`. Priority 1 (low) to 6;
2 is normal.

- **Proposals and decline memory.** A session's proposal states why. A title the owner
  declined in a project (case and spacing ignored) cannot be filed there by a session again.
- **Waits.** `add --after` and the owner's `wait` make an item wait on others; a chain that
  comes back to the item is refused. An item waits while one of them is not done, and lists
  their summaries once they are. Deleting or declining an item open items wait on is refused.
  A session may wait only on items it can see.
- **Across projects.** A session files in its own project's list. Filing in another one
  queues the item there (handed across) and counts against
  `Settings::cross_project_open_max` open items per (filing project, target project). A
  session outside every registered project files only blocker reports.
- **Blocker reports.** What a session could not do because of the setup goes, with its
  reason, to `Settings::blocker_project`, queued; without one configured, it is refused.
- **Revisions.** Title, notes, kind, waits and images are intent: each change increments
  `intent_revision`, and an edit naming an older one is refused. Status, priority, queue and
  summary are state (`state_revision`), and keep the intent.
- **Images and drafts.** The owner attaches PNG, JPEG, GIF or WebP images (type read from
  the bytes, 4 MiB, 20 per item); `[Image N]` in the notes names image N. A draft keeps
  unsaved form text for a project's new item or for an item, and says when the item's intent
  changed since the form was opened.

Item ids are never reused: a deleted item's id names nothing afterwards.

- **Who finished it.** `finish` keeps the session that was doing the item (`finished_by`);
  `reopen` clears it. `session_work` answers, per session, how many items it is doing and
  how many it finished: the daemon's "finished" label for a session reads it (#157, decided
  in #201).

## Who may do what

The owner does everything. A session needs `change-state` over itself (granted at launch,
`docs/decisions/2026-10-31-31-self-state-grant.md`); with it, it sees its own project's list
and what it filed, files (`add`, `propose`, `report_blocker`), starts its project's items,
finishes the one it is doing and deletes what it filed while it is `proposed` or `todo`.
Approving, declining, editing, priorities, the queue, waits, images and drafts are the
owner's.

## Dependency edges

- Depends on: `cockpit-turso` (transactions, migrations) and `cockpit-authorization`
  (every session request is evaluated).
- Never on `cockpit-scheduling` (`capabilities -> scheduling`), `cockpit-workflows`, the wire
  types, a client or `cockpit-sessions`: the daemon identifies the sender, finds its project,
  reads the owner settings and converts to wire types
  (`cockpit-workspace-rules/tests/crate_graph.rs`, rule `tasks -> workflows/sessions/wire types`).
- Used by: the daemon.

## Tests

- `tests/items.rs`: statuses, proposals, decline memory, waits, caps, blocker reports and
  revisions on a real state database.
- `tests/attachments.rs`: images and drafts.
- `tests/finished_by.rs`: the finishing session and `session_work`.
- `cockpit todo` end to end: `cockpit-cli/tests/todo.rs` (owner) and
  `cockpit-daemon/tests/tasks.rs` (agent socket).
