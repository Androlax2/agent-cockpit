# Resume refuses a conversation alive elsewhere, restart takes over an unknown status, and "finished" is task evidence

Issue #157. The finished label decided by Theo in #201 (option A); the rest decided by the
worker, with the reasons below.

- **"Alive anywhere" means any Cockpit session's execution of the same provider conversation.**
  Report 04 identifies an execution by its supervisor identity, never a PID, so `resume`
  observes, through `cockpit-processes`, the latest execution of every other session holding
  the conversation, and refuses while one is occupying (a failed launch may leave one whatever
  its session's state says). The transaction that moves the session to `launching` also
  refuses while another holder is `launching`, `running` or `stopping`, so two resumes of
  sessions sharing a conversation cannot both pass. The session's own previous agent is still
  stopped first, as #31 decided. Processes Cockpit did not launch (the provider run by hand)
  have no identity Cockpit can verify and are out of reach.
- **Restart starts from `running`, `launching` and every `stopping`.** Those are the states in
  which the agent's status is unknown after an error or a crash; the stop observes the agent's
  execution (`terminate` records what it sees), stops it if it still runs, then launches the
  same conversation. `asleep`, `ended` and `failed` stay refused with their verb (`wake`,
  `resume`): their status is known. `start` now holds the new session's lock through its first
  launch, so a restart can never take over a launch still under way.
- **Finished is computed by the daemon, never stored on the session.** `cockpit-tasks` keeps
  `finished_by` (the session doing the item when it was finished, cleared by `reopen`); the
  daemon labels a session finished when it is not launching, stopping, failed or ended, is doing no item and
  finished at least one. An agent's own report (waiting, idle) never counts.
- **`session-end-finished` ends the finished sessions one by one.** One failure does not stop
  the others; the reply lists both. Each is checked again just before it ends. The basic TUI
  folds them into one group out of the selectable list (except one waiting on the owner,
  which stays listed), and `X` (confirmed with `y`) sends the verb with the ids the owner saw
  (`only`), so a session that finished since is never ended unseen.
- **Rejected:** a finished state in `cockpit-sessions` (it would need `sessions -> tasks` and
  duplicate the task list's truth); refusing resume on another holder's `asleep` state (no
  process is alive, and the issue is about processes).
