# Only a launch made to give a session work drops its finished label

Issue #269. Decided by Theo's coordinator in #312 (option A).

- **Working launches: start, resume, the owner's `session restart`, every wake.** A wake by
  open, message, todo or phone means someone is about to talk to the session, so it drops
  the label like the owner's own wake.
- **Upkeep launches keep it: the restore on daemon start, `restart --all` and its deferred
  start-file restart.** They relaunch the same conversation with no new input, so a finished
  session stays folded after a reboot or a start-file change.
- **One nullable column, `session_launches.working_order`, set by the launch's caller:** its
  commit order for a working launch, `NULL` for upkeep. The launch kind alone cannot tell:
  `restart` covers the owner's restart and the start-file restarts.
  `Session::working_launch_order` is the latest working launch's commit order (#273); the
  daemon compares the latest finish with it.
- **A launch or restart a crash interrupted counts as working when the next daemon finishes
  it.** Who asked for it is not stored, so a session left `launching` or stopping to restart
  drops the label, the safe direction (`X` skips the session); an interrupted `restart --all`
  loses the label too. Only a session found running with its agent gone restores as upkeep.
- **Launches recorded before the migration count as working, restores excepted.**
- **Rejected:** B, skipping only `restore`, and C, accepting the loss, in #312.

Evidence: `crates/cockpit-daemon/tests/finished_label.rs`.

## Revisited 2026-10-31 (#226)

Decided by the worker within the rule Theo's coordinator settled in #201 (option A): the
issue's two options give the same label, and this one needs no write outside the finish. The
earlier decision of this story: a session is finished only by an item it finished after its
latest launch. Two parts are replaced: "every launch kind counts" by the working launches at
the top, and the finish time compared with `launched_at` by the commit order of the #273
section.

- **The label compares the item's finish time with `launch.launched_at`.** `cockpit-tasks`
  stores `finished_at` in the `finish` transaction (cleared by `reopen`) and `session_work`
  reports each session's latest finish; the daemon labels the session finished when that
  time is strictly after its latest launch. A restart, resume or wake to keep working
  therefore drops the label until new work is finished, and `session-end-finished` cannot end
  a conversation the owner just reopened.
- **Strictly after, in whole seconds.** A finish in the launch's own second does not count:
  the tie is resolved toward showing the session, never toward ending it. The #273 section
  below replaced the clock with commit order.
- **Items done before the migration have no finish time and never count after a launch.**
  `changed_at` is not a finish time: an edit or priority change after the finish moves it,
  so backfilling from it could label a just-resumed session finished.
- **Every launch kind counts, `restore` included.** A session reopened on daemon start,
  woken, or restarted by `restart --all` or a stall loses its label too: the safe
  direction. The top of this record narrows that to working launches (#269).
- **Rejected:** clearing the label on relaunch. It needs the daemon to write the task list on
  every launch (or `sessions -> tasks`, which the crate graph forbids) and stores a second
  copy of what the finish times already say.

## Revisited 2026-11-01 (#273)

Decided by the worker: the issue's option A names two ways (milliseconds or a shared
sequence); the shared sequence already exists, needs no clock, and cannot tie. The finished
label orders a finish and a launch by their commits, not by the clock.

- **`WriteTransaction::commit_order` (`cockpit-turso`):** the newest change-capture row when
  asked, at least every earlier commit's revision and below the transaction's own. The finish
  (`tasks_items.finished_order`) and the launch (`session_launches.working_order`) each store
  theirs; a finish counts when its order is greater, so one committed after the launch counts
  in the same second, and one committed before never does, whatever its time says. A clock
  stepping back changes nothing.
- **It stays valid across change-feed trimming and restores.** `discard_changes` always keeps
  the newest commit, so orders never repeat; a restore brings back the change rows with the
  rows that stamped them.
- **Data from before the migration:** launches get order 0, finishes none. A finish without an
  order never counts, so a session finished before the upgrade shows until it finishes again:
  the safe direction, as the #226 section chose for items without a finish time.
- **`finished_at` stays a time for people;** the label no longer reads it.
- **Rejected:** milliseconds, which still tie and follow a clock that may step back; B,
  accepting the tie (the #226 section).

Evidence: `crates/cockpit-turso/tests/commit_order.rs`, `crates/cockpit-daemon/tests/finished_label.rs`.
