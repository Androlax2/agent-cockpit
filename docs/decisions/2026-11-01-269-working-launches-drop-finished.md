# Only a launch made to give a session work drops its finished label

Issue #269. Decided by Theo's coordinator in #312 (option A).

- **Working launches: start, resume, the owner's `session restart`, every wake.** A wake by
  open, message, todo or phone means someone is about to talk to the session, so it drops
  the label like the owner's own wake.
- **Upkeep launches keep it: the restore on daemon start, `restart --all` and its deferred
  start-file restart.** They relaunch the same conversation with no new input, so a finished
  session stays folded after a reboot or a start-file change.
- **One column, `session_launches.is_working`, set by the launch's caller.** The launch kind
  alone cannot tell: `restart` covers the owner's restart and the start-file restarts.
  `Session::working_launch_order` is the latest working launch's commit order (#273); the
  daemon compares the latest finish with it.
- **A launch or restart a crash interrupted counts as working when the next daemon finishes
  it.** Who asked for it is not stored, so a session left `launching` or stopping to restart
  drops the label, the safe direction (`X` skips the session); an interrupted `restart --all`
  loses the label too. Only a session found running with its agent gone restores as upkeep.
- **Launches recorded before the migration count as working, restores excepted.**
- **Rejected:** B, skipping only `restore`, and C, accepting the loss, in #312.

Evidence: `crates/cockpit-daemon/tests/finished_label.rs`.
