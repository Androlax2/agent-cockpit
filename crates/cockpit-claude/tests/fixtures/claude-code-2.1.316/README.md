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
