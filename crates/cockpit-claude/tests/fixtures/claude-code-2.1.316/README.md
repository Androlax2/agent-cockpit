# Claude Code 2.1.316 hook payloads

One payload per hook event the adapter subscribes to, as Claude Code 2.1.316 writes it on
the hook's standard input. Captured from live runs on 2026-10-31 (#184): an interactive
session in tmux (startup, a prompt, a Bash permission prompt, the tool use, the turn's stop,
the idle notification after 60 seconds, `/compact`, `/exit`) and a `-p --resume` run.
Only the home directory, working directory and file owner were renamed (`/home/owner`,
`/work`, `owner`); field names, order and values are as written.

`stop-failure-rate-limit.json` is the exception: a rate limit cannot be provoked on demand,
so it is still written from the hook input schema the legacy hook consumed on 2.1.31x.

`tests/live_hooks.rs` re-runs the bridge against the installed Claude Code (ignored by
default). `cockpit-claude`'s `CONFORMANCE_VERSION` names this directory; a new pinned
version gets a new directory and the tests move to it.
