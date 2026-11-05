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
- **Toolchains, caches and Git (#859, decided by Theo: Q1 B, Q2 A, Q3 A).** The daemon detects
  the ecosystems whose markers sit at the checkout's root or in the session's start folder,
  and binds read-only the install roots the launch's `PATH` resolves their tools in. A marker
  it cannot read, such as a link loop an agent left, detects nothing and is logged rather
  than failing every later launch. Every cache variable of the table is set, not only a
  detected ecosystem's, so a tool used without a marker file still caches in the session's
  own `caches/` folder of its temp folder; the compiler wrappers are set empty and
  `GOFLAGS` gets `-modcacherw` added to the owner's own flags. The session's end removes
  read-only trees too, making their folders writable first, since the agent can unset that
  flag. Worker choices: tool configuration files and the session's Git configuration
  (`GIT_CONFIG_GLOBAL`) are written into `config/` of the session's folder of the mount
  sources and bound read-only, never into a folder the agent writes, so a link the agent
  planted can never turn the daemon's write onto another file; the session's end removes
  that folder. The Git configuration holds the owner's `user.name` and `user.email` as
  quoted values, read by one `git -C <checkout> config --global --includes` so an
  `includeIf` on the checkout applies, the checkout as `safe.directory` (not the owner's
  whole list) and an empty `credential.helper`. `GIT_CONFIG_NOSYSTEM=1` keeps
  `/etc/gitconfig`, whose host-scoped helpers an empty generic one does not clear, out, and
  every other `GIT_CONFIG*` variable of the launch is dropped. A launch under `off`, or one
  refused for want of a backend, gets none of this and runs no `git`.
- **Declarations refused at load (#861, decided by Theo: Q1 A, Q2 A, Q3 B).** The daemon
  checks every provider's `Needs` when it loads its providers, before any launch: a path a
  program reads or writes that overlaps a `~/` entry of `BASELINE_DENIED_READ` or
  `BASELINE_DENIED_WRITE`, or a folder another program declares `write`, refuses that
  program's declaration whole (`cockpit_sessions::refused_declarations`). The provider is
  then not wired: no session names it, and `torlek doctor` says why. Worker choice: the
  check runs at the daemon's start, where providers load, rather than at each launch, so a
  refused declaration shows before anything is launched; the owner's entries for unadapted
  programs (#84, #294) will call the same check when they load. Their source,
  `NeedsSource::OwnerEntry`, is in the spec for them; the program's hosts reach the spec
  with the proxy (#831). The executable is bound as its resolved file. `Unsandboxed`
  evidence is the owner's own act only: the `off` setting, or an adoption (#296).
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
