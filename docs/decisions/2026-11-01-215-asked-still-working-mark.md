# An agent that asked and still works is marked `?`, apart from one blocked on the owner

Issue #215. Decided by the worker, within decision 17 (the basic TUI).

- **`?` and "asked you, still working"** when the only thing waiting is the agent's own
  question (`question_source: agent`) and its latest state is `working`; `!` and "waiting on
  you" for a dialog, a run or guide request, or a question while it waits
  (`crates/cockpit-tui/src/board.rs`, `asked_while_working`).
- **`n` still opens it and it never folds into the finished group:** the question is open,
  and hiding it is worse than showing it.
- **A report without a question source** (a daemon before #182) counts as blocked.
- **Rejected:** dropping the asked-and-working session from `n` (the owner would miss a
  question the agent still needs answered).
