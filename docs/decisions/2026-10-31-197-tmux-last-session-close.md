# Closing a tmux server's last session waits until the server has exited

Issue #197. Decided by the worker, from a stress test on tmux 3.8.

- **`Server::close_session` waits.** After `kill-session` it polls `list-sessions` every
  20 ms until no server answers or the server holds a session again, and fails with
  `Error::EmptiedServerKeptRunning` after five seconds. tmux exits once its last session is
  gone, but only after its clients leave; a `new-session` in that window fails with "server
  exited unexpectedly". Without the wait, `restarting_the_only_session_never_reaches_an_exiting_server`
  in `crates/cockpit-tmux/tests/last_session.rs` failed on every run (16 servers × 10
  restarts); with it, 20 of 20 runs passed. Every caller gets the wait: the manager rebuild
  in `cockpit-daemon` and the session restart in `cockpit-sessions`. The manager's own wait
  is gone.
- **A failed tmux command while waiting counts as still exiting.** An exiting server can
  answer with "server exited unexpectedly" or "lost server"; only "no server running" or a
  stale socket ends the wait. A tmux that cannot be run at all fails the close at once. Any session ends it too, Cockpit's or not, since then the server stays up.
- **Rejected:** `set-option -s exit-empty off` in `CONFIG`. It removes the window for every
  caller, but leaves an idle tmux running after the last session and after every test that
  forgets `kill-server`, and a server started before the change still exits. The lifecycle
  stays as report 05 describes it: the server exits when it is empty.
- **Not covered:** a `new-session` from another caller, while one caller's close empties the
  server, can still reach it as it exits: the #202 section below.

## Revisited 2026-11-01 (#202)

Decided by the worker, from a stress test. A `new-session` that reaches an exiting tmux
server waits for its exit and tries again.

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
- **Rejected again:** `set-option -s exit-empty off`, for the reasons above.
