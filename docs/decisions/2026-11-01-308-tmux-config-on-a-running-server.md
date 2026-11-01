# `Server::open` applies Cockpit's tmux config to a server already running

Issue #308. Decided by the worker.

- **`source-file` on open.** tmux reads `-f` only when a command starts the server, so a
  running `-L cockpit` server kept the config of the Cockpit that started it until every
  session ended. `Server::open` now writes `CONFIG` and runs `source-file` on it; the daemon
  opens its handle at start, so an upgraded daemon applies the new config at once. Proven by
  `sessions::opening_applies_the_config_to_a_server_started_with_an_older_one` in
  `crates/cockpit-tmux/tests/`.
- **Each option is safe live.** All are global: `remain-on-exit`, `history-limit`,
  `default-shell` and `default-command` apply to panes opened afterwards; `status off` and
  `escape-time 0` change the running display and input only. A new option must be checked the
  same way before it goes into `CONFIG`.
- **No server, no start.** `source-file` starts no server; an exiting one ("server exited
  unexpectedly") is left alone, since the next server reads the new file as it starts.
- **Not covered:** an option removed from `CONFIG` stays set on a running server until it
  exits; re-sourcing only sets what the file names.
