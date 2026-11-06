# `doctor` blocks on an `agents.sandbox` the running daemon has not read, and names `config reload`

Issue #963. Decided by the worker: the issue offered a doctor message or a daemon that reloads
the setting by itself; the message needs no new behaviour in the daemon, and #354 already
ruled out reading the file at each use.

- **Readiness compares the two values it already has.** The daemon loads the owner's
  configuration on every readiness request and holds the `agents.sandbox` it applies
  (`Sessions::owner_settings`). When they differ, `cockpit_readiness::assess` replaces the
  `sandbox` line with one that names the written value and its source, the value still
  applied, and what still happens to agents (`host::unapplied_sandbox`).
- **It blocks (`missing-prerequisite`, `doctor` exits 1), in both directions.** What the owner
  wrote is not in force: with `required` written, agents still run unsandboxed while the file
  says otherwise. A blocking line is also the only kind `torlek init`'s readiness step shows.
- **The next action is `torlek config reload`, with the restart named second.** The issue asked
  for a restart, but the reload has applied `agents.sandbox` to later launches since #61
  (`crates/cockpit-daemon/tests/config_reload.rs`), and it keeps the running sessions.
- **A written `required` on a host without the sandbox is announced before the reload:** the
  next action adds that every launch is then refused, with the probe's reason.
- **A project's `execution environment` is `unknown` meanwhile.** It is assessed against the
  applied value, so its advice would be to write what the file already says; it points at the
  `sandbox` line instead. A project that expects Docker is not concerned.
- **A project file that does not read leaves nothing to compare.** Readiness has one merged
  configuration; `config` blocks on that file first, and the comparison shows once it reads.
- **`config show`'s note on stderr lists `agents.sandbox` too.** It named the three other
  settings a reload applies, which read as if the sandbox needed none.
- **Only `agents.sandbox` is compared.** The other reloaded settings (`agents.desktop_session`
  and the two `sessions.*` windows) keep their current reporting.
- **Rejected:** the daemon applying a changed `agents.sandbox` at each launch (#354 rejected
  reading the file at each use: an invalid edit would fail launches at runtime); an `available`
  line with advice (neither `doctor`'s exit code nor `init` would show that the owner's choice
  is not in force); a `systemctl` command in the next action (readiness does not know the
  profile's unit, and the reload is enough).

Evidence: `crates/cockpit-daemon/tests/readiness.rs`
(`a_sandbox_setting_written_while_the_daemon_runs_blocks_doctor_naming_the_reload`,
`after_the_reload_doctor_reports_the_written_sandbox_setting_as_in_force`) and
`crates/cockpit-readiness/tests/report.rs`.
