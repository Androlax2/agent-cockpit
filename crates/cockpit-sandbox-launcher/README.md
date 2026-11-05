# cockpit-sandbox-launcher

torlek-sandbox: starts one agent inside bubblewrap's namespaces, then applies Landlock and a
seccomp filter from memory before running it (#829).

Decisions D3 and D5 of [#546's report](../../docs/research/2026-11-05-546-own-sandbox-and-traffic-control.md):
bubblewrap is required and builds the namespaces and the mount view; this program applies
Landlock and seccomp itself, with the `landlock` and `seccompiler` crates pinned exactly and
`unsafe_code = "deny"` kept. It ships in `bin/` beside the daemon, which probes it at its
start and runs it in the agent's place under `agents.sandbox = "required"`.

## Verbs

- `torlek-sandbox run SPEC`: reads and removes the daemon's launch spec
  (`cockpit_sandbox::LaunchSpec`, written owner-only in the session's mount-source folder),
  validates it, then runs bubblewrap with new user, mount, PID, network, IPC, UTS and cgroup
  namespaces and a mount view of only what the spec names, and waits. The program's exit
  status is the launcher's; 125 means the launcher refused or failed before the program ran.
- `torlek-sandbox stage2 POLICY ARGUMENT...` (internal): inside the namespaces, checks that each folder
  the plan pinned is still the one it saw (device and inode), applies Landlock at the probed
  ABI with `HardRequirement`, installs the seccomp filters, then runs the program in its
  place.
- `torlek-sandbox probe BWRAP`: the daemon's start-up probe. Prints
  `{"bubblewrap":"<version>","landlock_abi":<n>}`; a failure is the reason every launch under
  `required` is refused.

## Mount view

System folders (`cockpit_sandbox::SYSTEM_READ_ONLY`) read-only; the session's own home at the
owner's `HOME` path; the checkout, the session's folder and its temporary folder read-write;
the bridge programs, the instruction bundle and the agent socket's folder read-only; a fresh
`/tmp`, `/proc` and `/dev`. Inside the checkout, `.git/hooks`, `.git/config`, `.torlek` and
`.cockpit` are read-only (an empty read-only folder when absent), and so is a `.git` file
(a worktree's or submodule's); `.git/modules`, `.envrc` and the editors' task files are
read-only when present; `.env` files and the host's denied paths are masked, a link's
target with it. The terminal or file each standard stream already is can be reopened
through `/dev/stdout` and `/dev/stderr`.
There is no network but `lo` until the session proxy (#831).

## seccomp

A deny list, built in memory: tracing, mounts and the new mount API, namespace creation
(`clone3` gets `ENOSYS` so callers fall back to `clone`, whose namespace flags are refused),
kernel modules, keys, BPF, `io_uring`, `TIOCSTI`/`TIOCLINUX`, and socket families other than
Unix, IPv4, IPv6 and netlink. On x86_64 the x32 aliases are refused too.
