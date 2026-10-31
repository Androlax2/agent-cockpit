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
  server, can still reach it as it exits: #202.
