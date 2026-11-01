# A new conversation starts afresh, and only a stated report answers a dialog

Issues #213, #216, #225, #250. The conversation reset was decided by Theo in #311 (option A);
the rest by the worker, from the options the issues list. Amends the clearing rules of
`2026-10-31-182-question-sources.md`, `2026-10-31-183-turn-outcome.md` and the wire point of
`2026-10-31-214-dialog-stacking.md`.

- **A new conversation is treated like a new launch.** Once a session follows the
  conversation that replaced an announced end (`--conversation-ends`, #219: `/clear` and an
  in-session `/resume` alike), the previous conversation's own question, its dialog and the
  question beneath it, and its turn outcome are gone. The label and doing-now stay: they
  describe the session's task. No new wire flag: the bridge already announces the change, and
  `conversations::check` says when the session followed it. A provider naming its
  conversation for the first time is not a change. Rejected (#311 B): only `/clear` resets,
  which needs a reason on the wire and shows a question the resumed conversation never asked.
- **Only a report with a state or a turn's outcome answers a dialog.** A label, a doing-now
  or the agent's own question can come from a subagent or a parallel tool while the dialog
  still blocks the agent, so it leaves the dialog open (an agent question goes beneath it).
  Rejected: matching the dialog to the tool it gated by `tool_use_id`: Claude Code 2.1.316's
  permission `Notification` carries no tool id or name
  (`crates/cockpit-claude/tests/fixtures/claude-code-2.1.316/notification-permission-prompt.json`).
  Still open: a parallel sibling tool's `PostToolUse` `working` answers the dialog (#315).
- **The covered question is on the wire.** `SessionReport.covered_question` (absent from older
  daemons) carries the agent's question an open dialog covers; `cockpit agent reports` shows it
  as `also-waiting="…"`. The basic TUI shows no question text yet, so it shows none of it.

Evidence: `crates/cockpit-sessions/tests/report_conversation_reset.rs`,
`crates/cockpit-sessions/tests/report_dialog_stateless.rs`,
`crates/cockpit-daemon/tests/agent_question_resets.rs`,
`crates/cockpit-protocol/tests/agent_covered_question.rs`,
`crates/cockpit-cli/tests/agent_covered_question.rs`.
