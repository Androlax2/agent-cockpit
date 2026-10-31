# Agent reports name their conversation, and the daemon follows only a change the current conversation announced

Issue #219. Decided by the worker: the issue asks the daemon to refuse a report whose
conversation is not the session's while following `/clear` and resume, and the measured
payloads give one rule for both.

- **Evidence (2026-10-31, Claude Code 2.1.316, interactive in tmux).** `/clear` runs
  `SessionEnd` with `reason: "clear"` and the old `session_id`, then `SessionStart` with
  `source: "clear"` and a new one; an in-session `/resume <id>` does the same with
  `reason: "resume"` / `source: "resume"`. Both run from the launch's own process with its
  token (fixtures `session-end-clear.json` … `session-start-resume-switch.json`). `--resume`
  at launch keeps the id. A conversation moved to the background daemon starts with
  `source: "fork"` and a new id, announced by no end in the session it is reported as
  (measured in #184).
- **Rule.** The bridge sends `--conversation <session_id>` with every report, and
  `--conversation <old> --conversation-ends` (nothing else) for a `SessionEnd` of `clear` or
  `resume`. `cockpit-sessions` refuses (`invalid-argument`, field `conversation`) a report
  naming another conversation than the session's, unless the session's conversation
  announced its end in the same launch; that end lets exactly one new conversation in, which
  becomes the session's `conversation` (revision bumped), so restart, sleep and resume
  reopen what the owner sees. A report of the same conversation closes an open end; a new
  launch makes it stale. Reports without a conversation (an agent's own `--label`) are not
  checked. A session with no conversation recorded takes the first one reported.
- **Token stays out of argv.** The launch token is still only inherited through
  `COCKPIT_LAUNCH_TOKEN`; `--settings` and the report's arguments carry no secret.
- **Kept:** `CLAUDE_CODE_DISABLE_AGENT_VIEW=1` (#184) stays: it keeps conversations in the
  terminal Cockpit hosts; this check is the boundary when another path appears.
- **Rejected:** accepting any `SessionStart` with `source` `clear` or `resume` as a change,
  because a foreign conversation running `/clear` with another launch's environment would
  then take the session over; the announced end must come from the session's own
  conversation.
- **Rejected:** a new `Refusal` variant in `cockpit-agent-channel`: the sender is identified
  correctly, its request names the wrong conversation, which `InvalidRequest` already says
  with the field.
