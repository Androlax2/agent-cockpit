# The daemon names its tmux server by path, and refuses a missing TMUX_TMPDIR

Issue #845. Decided by the worker: the issue asked for `Socket::Path` always if `-L` could
reach another installation's server, and it can.

- **`tmux -L` is not isolation.** tmux takes `$TMUX_TMPDIR` only when that directory exists
  and otherwise falls back to `/tmp/tmux-<uid>`, silently. A daemon whose `TMUX_TMPDIR`
  was missing therefore reached the installed Torlek's server of the same profile name,
  which is how the acceptance runner killed the owner's sessions. Shown in a mount namespace
  whose `/tmp` was a scratch directory: `TMUX_TMPDIR=<missing> tmux -L probe` connects to
  `/tmp/tmux-1000/probe`. Two different profiles never share a server: their names differ.
- **The daemon resolves the socket itself and passes `-S`.** It is
  `$TMUX_TMPDIR/tmux-<uid>/torlek[-<profile>]`, or the same under `/tmp` when `TMUX_TMPDIR`
  is unset or empty: the path `-L` used, so an upgraded daemon still reattaches to the
  server an older one started. `tmux-<uid>` is created 0700 or checked, as tmux does
  (`crates/cockpit-daemon/src/instance.rs`). `cockpit-tmux` no longer has a `-L` form.
- **A set but missing (or relative) `TMUX_TMPDIR` stops the start** with
  `StartError::TmuxTmpdir`, before any tmux command: whoever set it meant isolation, and a
  fallback hides that it is gone. Test: `crates/cockpit-daemon/tests/tmux_socket.rs`.
- **Rejected:** the socket under the profile's runtime directory, which would isolate every
  scratch layout without `TMUX_TMPDIR`, because the running installation's server lives at
  the `-L` path: the first upgraded start would lose every running session's tmux.
- **Rejected:** creating a missing `TMUX_TMPDIR`, because a typo in it would start a second,
  silent server instead of saying so.
