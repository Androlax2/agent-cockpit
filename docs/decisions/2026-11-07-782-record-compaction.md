# A compaction is recorded beside the agent's report, never as activity

Issue #782. Decided by the worker, from `docs/research/2026-11-04-686-compaction-research.md`
(follow-up 2).

- **One `session_compactions` row per compaction** (sessions migration 14), written by
  `cockpit-sessions::report` in the same write transaction as every other report, so the
  transaction stays with its owner. The card shows the latest one of the session's current
  launch (`latest_compaction`), so a new launch does not show the last launch's.
- **A compaction report proves no activity.** `--compaction` is sent alone (plus
  `--conversation`), validated like `--conversation-ends`, and leaves the stored report
  (state, doing, question, outcome) as it was. Rejected: reporting `working` for the
  compaction, which would clear a turn's outcome and read as the agent doing work; and
  treating a post-compaction `SessionStart` as a wait, which is what made it an unknown
  event before.
- **Wire fields are additive.** `AgentReport.compaction` and `SessionReport.compaction` are
  optional and absent when unset, so an older client or daemon neither sends nor needs them.
  The provider contract carries `EventKind::Compaction` and `EventKind::ContextCompacted`
  (the `SessionStart` after one), neither of which is an observation.
- **Seconds are the daemon's clock** between the start and the end it received. An end whose
  start was lost is recorded without seconds; a start while another still runs fails the
  unfinished one. Rejected: seconds from the harness, which none of the three reports.
- **Sizes are best effort.** None of the three hook payloads carries tokens, and #479
  (context-size reporting) is not merged. Claude Code's size comes from its transcript (the
  last assistant message's input and cache tokens before; the `compact_boundary` after) and
  Codex's from its rollout (`token_count` before the compaction and the first after the latest
  `compacted` record). Both are written after the hook runs, so a size not there yet is left
  out, never guessed. A size read before the harness flushed this compaction's own record can be
  the previous compaction's; a freshness check needs each adapter to parse record timestamps,
  and is left to the first live capture. OpenCode's `ended` event carries tokens: before is input plus cache,
  after is output. The adapters read their own files; they share only
  `cockpit-provider-api::CompactionMoment`, as an adapter depends on nothing else.
- **Fixtures are written from the harnesses' documented payloads and schemas**, not captured:
  a capture needs a live compaction and its model spend. Each fixture README says so, and the
  live tests are the place to replace them.
