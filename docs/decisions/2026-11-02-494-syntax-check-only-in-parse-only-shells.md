# The approval syntax check gives a command only to shells known to parse it without running it

Issue #494. Decided by Theo: recognize resolved shell names in any installation directory,
so Nix and Homebrew shells retain the syntax check; unknown shells wait for approval.

- **A shell is judged by the program its path resolves to.** `Runner::check_syntax` follows
  the approval shell's symlinks (`std::fs::canonicalize`) and checks the file name against
  `PARSE_ONLY_SHELLS` in `crates/cockpit-approvals/src/pty.rs`: `sh`, `bash`, `dash`, `zsh`,
  `ksh`, `ksh93`, `mksh`, `fish`, the shells whose `-n` is POSIX `set -n` or fish's
  `--no-execute`. So `/bin/sh` counts as whichever of dash or bash it is, `$SHELL=/usr/bin/nu`
  is nushell, and a link named `bash` that points at nushell is nushell. The path is
  resolved again at each check and the check starts the resolved file (under the link's
  name, so `sh` linked to bash stays in POSIX mode): a link retargeted between the judgement
  and the start cannot swap in nushell. The owner chose the shell, so its file name is
  trusted; agents cannot change it. There is no `/bin` or `/usr/bin` restriction.
- **Any other shell is not given the command before approval** (nushell, whose `-n` is
  `--no-config-file`, busybox, elvish, xonsh and anything unknown). The check answers
  `Syntax::NotChecked` and the request reaches the owner unchecked. A shell path that cannot
  be resolved fails the check, as a shell that could not be started did before.
- **The owner learns it on the request.** `RunRequestView::unchecked_shell` names the
  approval shell when the check was skipped; `cockpit approval show` and the TUI pane print
  "not syntax-checked: `<shell>` has no known parse-only mode" beside the other warnings
  (#156). It is computed from the daemon's shell when the view is built, not stored with the
  request: after a restart under another `$SHELL` it describes the new shell, which is the
  one the run would use, not the one that saw the ask. Storing it would be a schema change
  for a hint, since the check saves the owner an interruption and is not a safety gate.
- **Rejected:** checking with `/bin/sh -n` when the shell is unknown, because the run uses
  the owner's shell: valid nushell or elvish would be refused as POSIX errors, and a POSIX
  parse says nothing about how nushell reads it. **Rejected:** matching `$SHELL`'s own file
  name, because a link's name says nothing about the program it starts. **Rejected:**
  refusing every ask under an unknown shell, because the owner's choice of shell is
  legitimate and the check only saves the owner an interruption; it is not a safety gate.

Evidence: `crates/cockpit-daemon/tests/approvals.rs`
(`an_unknown_shell_receives_the_command_only_after_owner_approval`, edit and guide tests),
`crates/cockpit-approvals/tests/syntax_check.rs`.
