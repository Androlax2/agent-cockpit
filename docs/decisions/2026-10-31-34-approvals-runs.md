# Approvals: how approved commands run, and what waits for later issues

Issue #34. Implementation choices within report 17's settled design; none changes a
`decided` item.

- **The controlling terminal comes from a leader re-exec.** Making a pseudo-terminal the
  controlling terminal needs `setsid` and `TIOCSCTTY` in the child before `exec`, which a
  `pre_exec` hook could only do with `unsafe` (the workspace denies it). The daemon starts
  itself as `cockpit-daemon --run-leader <shell> -c <command>` on the terminal; that mode
  calls `setsid`, takes the terminal and `exec`s the shell, which keeps the pid, so the exit
  code is the command's own.
- **The shell is `$SHELL` when absolute, else `/bin/sh`**, the daemon's environment being the
  owner's. The legacy fish parse check becomes `<shell> -n -c <command>`, run before the
  owner is asked, outside any transaction; `run:` steps of a guide and an owner's edit are
  checked the same way.
- **Safe Git settings are `cockpit-projects`' whole `SAFE_CONFIG`**, passed as
  `GIT_CONFIG_COUNT/KEY/VALUE` with every inherited `GIT_*` variable and the launch token
  removed. Content filters and diff or merge drivers chosen by name are read from the
  configuration seen from the run's directory just before it starts (`Git::drivers_off`, the
  rule every daemon Git call follows) and reset too, user-wide ones such as Git LFS included.
  Limits: a command that `cd`s into another repository meets that repository's drivers; and
  `credential.helper` is reset, so an approved HTTPS push gets no stored credential.
- **A run that started is never reported as not run.** An exit that could not be observed,
  or an end that could not be recorded, marks the run `outcome-unknown`; a run that could not
  be marked running is recorded `not-run`, never left approved.
- **The owner's input to a prompt comes from standard input** (`cockpit approval input ID`),
  never from an argument, which shell history and process listings keep.
- **The asking agent's project is the registered checkout holding its working directory**
  (`/proc/<pid>/cwd`, read after the process tree was verified), deepest checkout first,
  until sessions record their checkout (#31). It only sets the "outside the project" flag.
- **Expiry**: an asking request or guide expires 15 minutes after its agent last asked about
  it; `cockpit ask run` polls every 500 ms and gives up with 124 after
  `COCKPIT_ASK_WAIT_SECONDS` (default 540, under a ten-minute tool call). An expired request
  answers 1, not 124: `--wait` cannot revive it.
- **Error codes**: a stale revision, digest or guide step is `stale-revision` (the owner
  reads again); an expired or decided request and input to a command that is not running are
  `conflict`.
- **Runs start outside the lanes, not through `cockpit-processes`.** The approval commits the
  run intent and replies; the run is a task of its own, so a long command holds no control
  worker. A restart cannot reattach it: `reconcile_runs` marks it `outcome-unknown`.
  Routing runs through `cockpit-processes` (reattach, scope containment) is left for a
  follow-up issue.
- **Left for later issues**: the full redacted output in the per-project answers directory
  waits for the sandbox mount (#61), so the answer carries the last 120 lines only; the
  approval pane, default Decline and ignored first keystrokes are TUI rules (#35); until
  then the owner decides with `cockpit approval` and `cockpit guide`.
