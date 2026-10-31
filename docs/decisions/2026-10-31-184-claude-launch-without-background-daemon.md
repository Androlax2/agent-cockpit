# Claude Code launches run with its background daemon turned off, so hooks keep their launch token

Issue #184. Decided by the worker: the bridge's session identity is only the inherited
`COCKPIT_LAUNCH_TOKEN`, and live runs on Claude Code 2.1.316 showed one path that breaks it.

- **Evidence (2026-10-31, 2.1.316).** A direct launch, interactive in tmux or `-p`, runs every
  hook as a child of the session's own process with the launch's environment: the token
  arrives unchanged (`crates/cockpit-claude/tests/live_hooks.rs`). `claude --bg`, `/background` and the exit menu's "Move to background"
  instead continue the conversation in a shared daemon (`claude daemon run`, pre-started
  `bg-spare` processes). Those processes carry the environment of whichever launch started
  the daemon: a second `--bg` session with token B, and a conversation moved there from a
  launch with token T, both ran their hooks with the first launch's token A. The moved
  conversation also gets a new `session_id` (`SessionStart` with `source: fork`).
- **Choice.** `ClaudeCode::launch` sets `CLAUDE_CODE_DISABLE_AGENT_VIEW=1`. Measured: with it,
  `/bg` and `/background` are not commands and the exit menu offers only "Exit and stop
  tasks" and "Stay"; without it both paths exist. A Cockpit session therefore cannot leave
  the terminal Cockpit hosts, which it could not usefully do anyway.
- **Rejected:** the bridge detecting the daemon from its environment, because the daemon's
  markers (`CLAUDE_CODE_SESSION_KIND=bg`, `CLAUDE_BG_BACKEND`) are removed before hooks run.
- **Rejected:** the bridge refusing a payload whose `session_id` differs from the launch's,
  because the bridge has no state to follow a legitimate change of conversation (`/clear`,
  not measured here; `--resume` keeps the id); the daemon owns that check once reports
  carry the conversation.
- **Not covered:** a `claude --bg` session someone starts outside Cockpit is not a Cockpit
  launch and runs without the bridge.
