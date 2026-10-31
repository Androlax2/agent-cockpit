# A session is finished only by an item it finished after its latest launch

Issue #226. Decided by the worker within the rule Theo's coordinator settled in #201 (option A):
the issue's two options give the same label, and this one needs no write outside the finish.

- **The label compares the item's finish time with `launch.launched_at`.** `cockpit-tasks`
  stores `finished_at` in the `finish` transaction (cleared by `reopen`, backfilled from
  `changed_at` for items already done) and `session_work` reports each session's latest
  finish; the daemon labels the session finished when that time is strictly after its latest
  launch. A restart, resume or wake to keep working therefore drops the label until new work
  is finished, and `session-end-finished` cannot end a conversation the owner just reopened.
- **Strictly after, in whole seconds.** A finish in the launch's own second does not count:
  the tie is resolved toward showing the session, never toward ending it.
- **A `restore` launch counts as a launch.** A session reopened on daemon start loses its
  label too; the safe direction, followed up in #269.
- **Rejected:** clearing the label on relaunch. It needs the daemon to write the task list on
  every launch (or `sessions -> tasks`, which the crate graph forbids) and stores a second
  copy of what the finish times already say.
