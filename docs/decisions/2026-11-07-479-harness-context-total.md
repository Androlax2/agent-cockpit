# The harness's own context rides on the turn's end report and resets with its conversation

Status: accepted

Issue #479. Decided by the worker, from the formulas in
`docs/research/2026-11-04-686-compaction-research.md` ("Context size to Torlek (#479)").

- **Two columns on `session_reports`** (sessions migration 16: `context_tokens`,
  `context_window`), written by `cockpit-sessions::report` in the report's own write
  transaction. A report without an outcome keeps the last context; a turn's end without one
  clears it, as that size is no longer the latest turn's; a new launch or a new conversation
  (`/clear`, an in-session `/resume`) starts without one, because its harness context starts
  again. `current_context` reads it only for
  the session's latest launch, so a brief before the new launch's first report says "not
  reported yet". Rejected: a table of its own, as compactions have, because only the latest
  value is kept and the report row already carries the launch and conversation rules.
- **A context comes only with a turn's outcome** (`--context-tokens` needs `--outcome`, checked
  by the CLI and again by `cockpit-sessions`). It is the harness's size at a turn's end, and a
  lone size would refresh `reported_at` without saying what the agent does. A refused size
  refuses the whole report, so the counts are checked to fit a column and a window is never
  zero; adapters send only what the harness wrote.
- **Each adapter reads its harness's own count and sends what it has.** Claude Code: the
  transcript's last main-chain model call (input plus cache read and creation), no window.
  Codex: the rollout's last `token_count` (`last_token_usage.input_tokens`) with
  `model_context_window`. OpenCode: the turn-ending assistant message's `tokens` (input plus
  cache read and write), no window; all zero means the provider reported no usage, never a
  context of zero, and OpenCode usage without `input` is none either. A compaction recorded
  after the last model call gives no size, as that call's size is no longer the context. As
  for compactions (decision 782), a transcript or rollout not flushed yet can give the
  previous model call's size; a failed turn reports its last real model call's.
- **`brief --measure` prints tokens beside bytes, never converted.** The daemon adds
  `harness_context` to the brief's reply (optional, absent from an older daemon); the owner's
  `SessionReport` wire is unchanged.
