# A `new-session` that reaches an exiting tmux server waits for its exit and tries again

Issue #202. Decided by the worker, from the stress test below.

- **`Server::new_session` retries.** When tmux answers "server exited unexpectedly" or
  "lost server", the server closed the connection as it exits: it was empty, so it created
  no session. The call waits as `Server::close_session` does (until no server answers or the
  server holds a session again), then runs the same `new-session` again, until it succeeds,
  fails another way or five seconds have passed; then the last failure is returned. Other
  callers can empty a fresh server again meanwhile, so one retry is not enough. This covers
  every caller and every process, which a lock inside `Server` would not. Proven by
  `a_session_opened_while_another_caller_empties_the_server_always_starts` in
  `crates/cockpit-tmux/tests/last_session.rs` (5 of 5 runs failed without the retry, 10 of
  10 passed with it, tmux 3.8).
- **Rejected:** a lock serializing close and new-session inside `Server`: it covers only
  handles in one process, and the CLI, the daemon and a restarted daemon are several.
- **Rejected again:** `set-option -s exit-empty off`, for the reasons in
  `2026-10-31-197-tmux-last-session-close.md`.
