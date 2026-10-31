# A dialog stacks on the agent's own open question instead of replacing it

Issue #214. Decided by the worker, as the issue's fix states; amends the "one open question
per session" point of `2026-10-31-182-question-sources.md` (the clearing rules stay).

- **The agent's question waits beside an open dialog.** `session_reports.covered_question`
  (`cockpit-sessions` migration 5) holds the agent's own question while `needs_input` shows
  the dialog; the schema allows it only while `question_source` is `dialog`. Once any report
  answers the dialog, the agent's question is the open question again, with source `agent`.
- **Each source keeps its clearing rule.** A new prompt or a newer launch clears the agent's
  question, covered or not; a new `--needs-input` replaces it; a second dialog replaces the
  first and keeps the agent's question beneath.
- **The wire is unchanged.** Owners still read one open question with its source; the
  covered question stays inside `cockpit-sessions` until it shows again.
- **Rejected:** rebuilding the table into `agent_question` and `dialog` columns: same
  behaviour, but it rewrites every row and migration 4's schema guarantees for no reader.

Evidence: `crates/cockpit-sessions/tests/report_dialog_stacking.rs`,
`crates/cockpit-daemon/tests/agent_dialog_stacking.rs`.
