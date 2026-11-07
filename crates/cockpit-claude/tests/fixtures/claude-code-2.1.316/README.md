# Claude Code 2.1.316 hook payloads

One payload per hook event the adapter subscribes to, as Claude Code 2.1.316 writes it on
the hook's standard input. Captured from live runs on 2026-10-31 (#184): an interactive
session in tmux (startup, a prompt, a Bash permission prompt, the tool use, the turn's stop,
the idle notification after 60 seconds, `/compact`, `/exit`) and a `-p --resume` run.
Only the home directory, working directory and file owner were renamed (`/home/owner`,
`/work`, `owner`); field names, order and values are as written.

`session-end-clear.json`, `session-start-clear.json`, `session-end-resume.json` and
`session-start-resume-switch.json` come from a second interactive run (#219): `/clear`, then
`/resume <id>` of another conversation inside the same session. Each change is a `SessionEnd`
of the old conversation followed by a `SessionStart` of the new one, from the same process
with the same launch token.

The two `stop-failure-rate-limit-*.json` payloads are the exception: a rate limit cannot be
provoked on demand, so they are written from 2.1.316's code (#212): its `StopFailure` hook
input (`error`, then the optional `error_details` and `last_assistant_message`) and the
message its API-error mapping sets for a spent 5-hour window (`plan-window`) and for a
server-side 429 (`transient`). Both carry `error: "rate_limit"`.

`tests/live_hooks.rs` re-runs the bridge against the installed Claude Code (ignored by
default). `cockpit-claude`'s `CONFORMANCE_VERSION` names this directory; a new pinned
version gets a new directory and the tests move to it.

`pre-compact.json` and `post-compact.json` (#782) are the exception too: written from the
documented `PreCompact` (`trigger`, `custom_instructions`) and `PostCompact` (`trigger`,
`compact_summary`) inputs (code.claude.com/docs/en/hooks, read 2026-11-07), not captured,
because a compaction needs a model reply. `transcript-compaction.jsonl` is a made-up
transcript tail with the fields the adapter reads: an assistant record's `message.usage`,
a sidechain record that must be skipped, and a `compact_boundary` record's `compactMetadata`
(field names from report 686's measurement appendix).

`transcript-turn.jsonl` (#479) is a made-up transcript tail of one turn, with the fields the
adapter reads at `Stop` and `StopFailure`: the main-chain assistant records' `message.usage`
(the last one is the turn's end) and a sidechain record that must be skipped. Its field names
are those of `transcript-compaction.jsonl`.
