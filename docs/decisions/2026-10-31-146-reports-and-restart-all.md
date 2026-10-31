# Agent reports move into cockpit-sessions; restart-all restarts only sessions on old start files

Issue #146. The restart trigger was decided by Theo in #177 (option A); the migration shape
by the worker, for the reason given.

- **Reports belong to sessions.** `cockpit-sessions` owns `session_reports`, the `report`
  verb's validation and merging, and its authorization check; `cockpit-agent-channel` keeps
  only identifying the sender (`identify`). Refusals keep the channel's `Refusal` shape, so
  every agent request is refused alike.
- **The receiving owner moves the rows.** Sessions' migration 2 creates `session_reports`,
  copies `agent_channel_reports` and drops it. Migrations apply set by set, the agent
  channel's before sessions', so a drop in the channel's own version 2 would run before the
  copy. Sessions' migrations therefore need the channel's applied first, as the daemon does.
  Rejected: dropping the rows (reports are small, but a migration that loses data sets a bad
  precedent).
- **Each launch records its start files** (`session_start_files`, one row per file, the
  `cockpit-config` snapshot) before the agent runs, so a change between the two reads as
  changed. A launch from before the table has none: every file its provider reads counts as
  changed. The declared list is read at daemon start.
- **`restart --all` is the only trigger (#177 A).** It restarts the running sessions whose
  start files changed: now when the agent's latest report since its launch says it waits at
  its prompt with no question, otherwise it records a restart request and the periodic pass
  restarts the session once it does. A session that never reports is deferred, never cut.
  Any launch clears the request, and so does a move to ended or failed. The decision is made
  under the session's lock on a fresh read of the session and its report. Rejected: automatic
  restarts (B), they restart an agent the owner may be reading.
- **A report names the launch it was sent from** (`session_reports.launch_id`, the session's
  latest launch when it reported). Restart and idle sleep only trust a report of the current
  launch; comparing report and launch times in seconds let a report from the previous agent,
  sent in the launch's second, count for the new one.
- **Wake paths share one transition.** `wake(id, cause)` is the owner's strict verb;
  `ensure_awake(id, cause)` is for callers that need the agent running (open, message, todo,
  phone) and answers a running or launching session as it is. The cause is the reason of the
  move to `launching` in the transition history.
- **`cockpit-config` depends on no product crate**, now a crate-graph allow-list rule, since
  sessions depend on it.
