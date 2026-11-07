# Typed approval input reaches a command only while its terminal hides it, unless sent shown

Status: accepted

Issue #1160. Decided by Theo (option B of the worker's proposal).

- **Input is typed only while the terminal's echo is off.** An approved command runs on a
  pseudo-terminal whose output is what the agent reads and `approvals_runs.output` stores, so
  a line typed while echo is on lands there, a password included. `Runner::send_input` reads
  the terminal's settings on the master side and refuses with `TerminalEchoes` (wire
  `conflict`, naming the echo) before writing. A password prompt (`sudo`, `ssh`, `gpg`) turns
  echo off while it reads, so `sudo` inside `torlek ask run` keeps working.
- **One named opt-out, `shown`.** `torlek approval input ID --shown`, or Enter on a line the
  owner revealed with Tab in the TUI, types a visible answer (`y`, `[Y/n]`, `read`) whatever
  the echo: the owner accepts that it shows in the output. The TUI has no second path: the
  reveal is the opt-out. The field defaults to off on the wire, so an older client gets the
  echo rule.
- **Residual race.** A program could turn echo on between the check and the write; a password
  prompt does so only after it has read its line.
- **The owner purges one run's stored output with `torlek approval purge-output ID`.** It
  clears `output` as retention does after `database_retention_days` (#406) and keeps the rest
  of the request as the audit record. The daily backups (`database_backups_kept`, 7) still
  hold it until they age out; delete the backups dated after the run to remove it sooner.
  What the agent already received stays in its own transcript, outside Torlek.
- **The redactor no longer counts `password for <user>:`.** The `.netrc` rule took `for` in
  `[sudo] password for theo:` for a value, a false "1 secret redacted".
- **Rejected:** refusing `sudo` at ask time, because matching the command text is bypassed by
  a script or quoting and the echo rule is needed anyway; refusing every echoed answer with no
  opt-out, because prompts such as `Continue? [y/N]` could no longer be answered.

Tests: `crates/cockpit-daemon/tests/approvals/typed_input.rs`,
`crates/cockpit-daemon/tests/approvals.rs` (`the_owner_answers_a_visible_prompt_with_shown_input`),
`crates/cockpit-approvals/tests/redaction.rs`.
