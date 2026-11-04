# Requested views follow the change feed through one `view-stamp` per area, with a 5 s fallback poll

Issue #706. Decided by Theo (option A of the worker's question); the details by the worker.

- **One feed kind, `view-stamp`, not an entity per view.** Projects, agent reports, the approval
  queue, the task list and the workflows are answered by request, and the approval queue and the
  workflow packet are built from git and the runner's memory, so they are not entity-shaped. The
  feed carries only `{changed_at}` per area (`ViewArea::id` is the entity id); the TUI asks again
  for the area whose stamp moved. The answer stays the truth.
- **The daemon derives the stamps from the change capture, not from the views.**
  `cockpit-daemon`'s `view_stamp_feed` reads which tables each commit touched
  (`ReadTransaction::commits_after`) and maps them to areas by name (`area_of`). A new table that
  an area reads must be added there; until then the 5 s poll still shows it. At start every area
  is stamped, as commits may have touched them while the daemon was down.
- **The TUI keeps a slow poll of the same requests every 5 s.** A stamp cannot see what no commit
  records: the runner's memory (`waiting_for_input`), and a question's hold running out with the
  clock. Stamp-triggered requests count as refreshes, so a slow daemon still gets no pile.
- **Old TUIs and unknown areas are safe.** `Board::apply` skips a kind or an area it does not
  know; the closed `Change` enum is untouched.
- **Rejected:** a feed kind per view (projects, reports, approvals, tasks, workflows) because the
  approval and workflow views cannot be published as entities without moving their git and
  runner reads into the publisher. **Rejected:** only slowing the poll, which keeps views up to 5 s
  late.
- **Measured** with `crates/cockpit-tui/tests/e2e/wakeups.rs` (ignored): with the TUI open and a
  1 s poll the daemon had about 7 more scheduler timeslices and 10 more voluntary context
  switches per second than alone; with the refresh disabled the difference was nil.
