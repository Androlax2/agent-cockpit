# The owner sets the idle window and asleep expiry in whole minutes and days, with minimums

Issue #144. Decided by the worker: the issue asks to keep the current cockpit's minimum.

- **`[sessions] idle_sleep_after_minutes` (default 60, at least 30) and `asleep_expiry_days`
  (default 7, at least 1) in the owner's `config.toml`.** Whole units match the current
  cockpit's `idle-sleep-after` minutes; `u32` makes a negative value a parse error with its
  line instead of a wrapped huge window. A value below the minimum fails the load, and with it
  the daemon's start, instead of falling back to the default as the current cockpit does:
  the owner sees the mistake.
- **Owner-only.** A project file that sets `[sessions]` fails: how long the owner's agents
  stay up is not a property of a repository.
- **`cockpit-config` holds the defaults the daemon uses; `cockpit_daemon::session_settings`
  maps them into `cockpit_sessions::Settings`.** No new crate edge: the daemon already
  depends on both. `config show` prints the effective values with their source.
- **The daemon loads the whole owner configuration at start and refuses to start on any
  error in it**, not only in `[sessions]`, after taking the instance lock and starting its log
  so the reason is recorded. A partial read would run with settings the owner did not write.
- **An expiry of at least one day.** Shorter would end a session put to sleep for the night.
- **Rejected for now: "disabled".** The current cockpit can turn idle sleep off; that needs
  a lifecycle change in `cockpit-sessions`, which #157 is editing. Left to its own issue.
