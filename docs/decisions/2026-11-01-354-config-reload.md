# The owner's `cockpit config reload` applies changed session settings to a running daemon

Issue #354. Decided by Theo in #364 (option B), 2026-11-01.

- **An owner-only verb, `cockpit config reload` (wire `config-reload`).** The daemon re-reads
  the owner's file and applies `agents.desktop_session`, `sessions.idle_sleep_after_minutes`
  and `sessions.asleep_expiry_days` to every later launch and sessions pass
  (`Sessions::apply_owner_settings`). It prints each setting, with `# was <old>` where it
  changed. Reloads run one at a time, so one that read an older file never lands after a
  newer one. The agent socket refuses it (`not-authorized`). Proof:
  `crates/cockpit-daemon/tests/config_reload.rs`.
- **An invalid file is refused (`invalid-argument`, exit 1) and the settings in force stay:**
  nothing is half-applied, and no default the owner did not choose replaces a value
  (`an_invalid_owner_file_is_refused_and_the_settings_in_force_stay`).
- **Running agents keep the environment they started with;** an idle window or an expiry
  counts with the new value from the next sessions pass.
- **`config show` points at it on stderr,** so its stdout stays one `key = value  # source`
  line per value for scripts. Start files still count from the next daemon start, and it
  says so too.
- **The values travel as the owner's file writes them** (TOML text, `SessionSettings::rendered`
  in `cockpit-config`), so `config show` and `config reload` print them alike.
- **The end-to-end tests' shortened idle window (`Serve::idle_sleep_after`) does not survive
  a reload;** only that test daemon sets it, and none of its tests reloads.
- **Rejected:** `config show` asking the daemon and printing both values (the change still
  needed a restart), and reading the file at each use (an invalid edit would fail launches at
  runtime, or need a silent "keep the last valid" fallback).
