# Codex sessions run a private app-server, and the adapter delivers submit and cancel through it

Issue #48, after #407 (decided by the coordinator: option A). Details below were measured on
Codex 0.161.0 with a fresh `CODEX_HOME`.

- **A private app-server per session.** With the bridge, a Codex session runs
  `cockpit-codex-session`, which starts `codex app-server --listen unix://<session dir>/codex.sock
  --enable hooks --config <hook>…` and then the TUI against it, `codex [resume] --remote
  unix://<socket>`. Both run in the session's terminal and sandbox with the launch's
  environment, so hooks keep the launch token. The hooks and their launch-scoped trust go on
  the server, which is where Codex runs them. When the TUI exits, the launcher sends the
  server `SIGTERM` (Codex's npm wrapper passes it on; end of input does not stop it), then
  `SIGKILL` after 5 seconds, to the wrapper's descendants too (read from `/proc` on Linux).
  The server stays in the session's process group, so stopping the session stops it too.
- **The provider contract gains `LaunchRequest::session_dir` and `Provider::deliver`.** The
  session's directory is `sessions/<id>` in the daemon's socket directory, made 0700 by
  `cockpit-sessions` before each launch. `deliver` takes the conversation as well as the
  directory: the TUI loads helper threads of its own on the same server, so "the session's
  thread" is ambiguous without it. It answers `StartedTurn`, `JoinedTurn` or `Interrupted`.
  Claude Code keeps keystroke delivery (`Provider::input`) until it has an equivalent.
- **Submit, steer and cancel.** A message to a thread that is not `active` starts a turn
  (`turn/start`). One to an active thread joins its turn (`turn/steer` with that turn's id);
  if the turn ended in between, a new turn starts. A cancel interrupts the turn under way
  (`turn/interrupt`) and is refused when none is. Whether a turn is under way comes from
  `thread/read`'s status first, because Codex refuses to list a thread's turns before its
  first message, and briefly (about 100 ms) after it; a refused listing is retried for 2
  seconds.
- **Failed turns come from `thread/status/changed`, not `turn/completed`.** Codex sends
  `turn/*` notifications only to the connection subscribed to the thread, which is the TUI,
  so another client never hears `turn/completed`. A failed turn leaves its thread
  `systemError`, which every connection hears. The launcher's watcher then reads the last
  turn and reports `waiting --outcome failed --outcome-reason <codexErrorInfo>`, or
  `--outcome quota-stopped` for `usageLimitExceeded`. Ephemeral helper threads and
  subagent threads (`parentThreadId`) are skipped. `Stop` now reports `--outcome completed`,
  so the adapter reports `outcome` as well as `submit` and `cancel`.
- **The WebSocket client is written in the crate.** On `unix://`, the app-server speaks
  JSON-RPC in WebSocket text frames. A client of about 300 lines on the standard library
  keeps `cockpit-codex` within `provider adapters -> provider-api only` and adds no network
  stack; `rustix` (already pinned in the workspace) sends the launcher's `SIGTERM`.
- **Rejected:** Codex's shared app-server, which runs hooks with the launch token of
  whichever launch started it (the #184 failure); a server outside the sandbox, which would
  lose the launch token; typing into the TUI, which #407 replaced; subscribing the watcher
  to the thread (`thread/resume`), which would make a second client of the conversation and
  change what Codex loads for it.
- **Known limits.** Codex puts the socket's real file in `/tmp/codex-daemon-<uid>/` and leaves
  a symbolic link at the path given, so a sandbox around the agent must share that directory
  with the daemon. Approval requests from the server are left to the TUI, which answers
  them; whether Codex also sends a turn started by `deliver` its approvals on Cockpit's
  short-lived connection was not measured without a signed-in account (#436). One delivery
  makes up to seven calls of at most 5 seconds each, plus 2 seconds of listing retries, so a
  stalled server holds it for about half a minute.
