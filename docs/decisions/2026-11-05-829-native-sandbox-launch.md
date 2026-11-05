# The native sandbox launcher runs in the agent's place, inside its execution's scope

Issue #829. Decided by Theo on #546 (D3, D5); the placement, hand-off and absent-path rules
below by the worker, with the reasons. Q1 and Q2 on #829 are open with the owner.

- **Placement (#834).** The daemon starts `torlek-sandbox run SPEC` where it started the
  agent: the hosted launch script runs it as the execution's program, inside the
  per-execution `systemd-run --user --scope` unit (`cockpit-processes`' `Containment`), or its
  process group without systemd. Bubblewrap, stage 2 and the agent are descendants in that
  same cgroup; bubblewrap's cgroup namespace only hides the path. #834 sets limits on that
  unit and changes no launch code. A check run (#862) gets the same placement.
- **Hand-off.** The spec is a JSON file the daemon writes owner-only (0600 in a 0700 folder)
  in the session's folder of the daemon's mount sources, through `Staged::write`, before the
  launch. The launcher reads it, removes it and validates it before anything
  runs, so a launch that started leaves none (#434); one a launch that failed earlier left
  holds no secret, and the session's next launch replaces it. The seccomp filter is built in memory in stage 2;
  nothing is written to a shared folder (#628). Rejected: the spec on the command line
  (visible in `ps`, length limits) and a pipe from the daemon (the hosted launch goes through
  tmux, which does not carry one).
- **The session's own home.** `$STATE/homes/<session>` is bound at the owner's `HOME` path:
  kept across the session's relaunches, removed at its end. The provider's declared reads
  are bound read-only on top when present; its credential files are masked (#858).
- **Absent protected paths (Q1, worker default).** `.git/hooks`, `.git/config`, `.torlek`
  and `.cockpit` get an empty read-only folder when absent, because Git and Torlek act on
  them outside the sandbox and an agent must not create them. `.envrc`, `.vscode/tasks.json`,
  `.vscode/launch.json` and `.idea/runConfigurations` are read-only when present and left
  creatable when absent: an empty folder there would break the owner's tools, and each runs
  only after the owner trusts it. The evidence states each rule.
- **The probe decides availability.** The daemon runs `torlek-sandbox probe` with the first
  `bwrap` on its `PATH` at its start; a failure, or Landlock below ABI 2, is the reason every
  launch under `required` is refused. Each launch enforces exactly the probed ABI's rights
  with `HardRequirement`, so a kernel change between probe and launch fails the launch.
- **`libc` as a direct dependency (Q2, worker default).** Linux-only, for syscall numbers and
  ioctl request constants in the filter; already in the tree through `seccompiler`. No
  `unsafe` call uses it.
- **Rejected:** applying Landlock and seccomp from the daemon before `exec` (the daemon
  launches through tmux, so the agent is not its child), and an allow-list seccomp filter
  (breaks toolchains on each new syscall; #546 M3).
