# An open question records where it came from, and its source decides what clears it

Issue #182. Decided by Theo in #200 (option C).

- **Two sources, two clearing rules.** `cockpit agent report --needs-input Q` is the agent's
  own question: it stays open through tool use, turn ends and idle, until a report with
  `--prompt` (the bridge sends it on UserPromptSubmit) or a report from a newer launch.
  `--dialog Q` is a dialog the provider bridge saw (permission prompt, elicitation): the next
  report of any kind clears it, since any later event means the dialog was answered. Fixes
  the stale question left by an answered dialog followed by a turn end.
- **One open question per session.** `session_reports.question_source` (`agent` or `dialog`,
  set exactly when `needs_input` is; `cockpit-sessions` migration 4) sits beside the text.
  A dialog arriving over the agent's own question replaces it; once the dialog is answered
  the agent's question is gone too. Accepted: the agent asks right before ending its turn,
  so a dialog in between is rare. Superseded by `2026-10-31-214-dialog-stacking.md`: the
  dialog now stacks on the agent's question, which is open again once the dialog clears.
- **The agent's question may stand while the session reports working.** A tool run after
  asking keeps the question; the owner's views show it from the moment it is asked.
- **Questions recorded before migration 4 become the agent's own,** the source that stays
  visible, so no question is hidden by the upgrade.
- **Rejected:** clearing on any `waiting` report (#200 A) and an explicit clear flag sent at
  turn end (#200 B): both clear the agent's own question at the end of the turn that asked it.

Evidence: `crates/cockpit-sessions/tests/report_questions.rs`,
`crates/cockpit-daemon/tests/agent_questions.rs`, `crates/cockpit-cli/tests/agent_questions.rs`,
`crates/cockpit-protocol/tests/agent_questions.rs`, `crates/cockpit-claude/tests/hooks.rs`.
