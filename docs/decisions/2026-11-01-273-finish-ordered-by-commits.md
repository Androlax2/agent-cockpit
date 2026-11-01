# The finished label orders a finish and a launch by their commits, not by the clock

Issue #273. Decided by the worker: the issue's option A names two ways (milliseconds or a
shared sequence); the shared sequence already exists, needs no clock, and cannot tie.

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
  the safe direction, as #226 chose for items without a finish time.
- **`finished_at` stays a time for people;** the label no longer reads it.
- **Rejected:** milliseconds, which still tie and follow a clock that may step back; B,
  accepting the tie (`2026-10-31-226-finished-since-latest-launch.md`).

Evidence: `crates/cockpit-turso/tests/commit_order.rs`, `crates/cockpit-daemon/tests/finished_label.rs`.
