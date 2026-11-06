# A new conversation starts afresh, and only a stated report answers a dialog

Issues #213, #216, #225, #250. The conversation reset was decided by Theo in #311 (option A);
the rest by the worker, from the options the issues list. Amends the clearing rules of
`2026-10-31-183-turn-outcome.md` and of the two revisited sections below (#182, #214).

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

## Revisited 2026-10-31 (#182)

Decided by Theo in #200 (option C). An open question records where it came from, and its
source decides what clears it.

- **Two sources, two clearing rules.** `cockpit agent report --needs-input Q` is the agent's
  own question: it stays open through tool use, turn ends and idle, until a report with
  `--prompt` (the bridge sends it on UserPromptSubmit) or a report from a newer launch.
  `--dialog Q` is a dialog the provider bridge saw (permission prompt, elicitation): the next
  report of any kind clears it, since any later event means the dialog was answered. Fixes
  the stale question left by an answered dialog followed by a turn end.
- **One open question per session.** `session_reports.question_source` (`agent` or `dialog`,
  set exactly when `needs_input` is; `cockpit-sessions` migration 4) sits beside the text.
  A dialog arriving over the agent's own question first replaced it; the #214 section below
  stacks it instead.
- **The agent's question may stand while the session reports working.** A tool run after
  asking keeps the question; the owner's views show it from the moment it is asked.
- **Questions recorded before migration 4 become the agent's own,** the source that stays
  visible, so no question is hidden by the upgrade.
- **Rejected:** clearing on any `waiting` report (#200 A) and an explicit clear flag sent at
  turn end (#200 B): both clear the agent's own question at the end of the turn that asked it.

Evidence: `crates/cockpit-sessions/tests/report_questions.rs`,
`crates/cockpit-daemon/tests/agent_questions.rs`, `crates/cockpit-cli/tests/agent_questions.rs`,
`crates/cockpit-protocol/tests/agent_questions.rs`, `crates/cockpit-claude/tests/hooks.rs`.

## Revisited 2026-10-31 (#214)

Decided by the worker, as the issue's fix states. A dialog stacks on the agent's own open
question instead of replacing it; this amends "one open question per session" from #182 (the
clearing rules stay).

- **The agent's question waits beside an open dialog.** `session_reports.covered_question`
  (`cockpit-sessions` migration 5) holds the agent's own question while `needs_input` shows
  the dialog; the schema allows it only while `question_source` is `dialog`. Once any report
  answers the dialog, the agent's question is the open question again, with source `agent`.
- **Each source keeps its clearing rule.** A new prompt or a newer launch clears the agent's
  question, covered or not; a new `--needs-input` replaces it; a second dialog replaces the
  first and keeps the agent's question beneath.
- **The wire is unchanged.** Owners still read one open question with its source; the
  covered question stays inside `cockpit-sessions` until it shows again. The third bullet
  at the top of this record later put it on the wire (`SessionReport.covered_question`, #225).
- **Rejected:** rebuilding the table into `agent_question` and `dialog` columns: same
  behaviour, but it rewrites every row and migration 4's schema guarantees for no reader.

Evidence: `crates/cockpit-sessions/tests/report_dialog_stacking.rs`,
`crates/cockpit-daemon/tests/agent_dialog_stacking.rs`.
