# A sandboxed Codex gets a socket folder of its session's own; the host's is never shared

Issue #446. Decided by Theo on 2026-11-01: option B.

- **The session's own folder at Codex's path.** Codex 0.161.0 keeps its app-server sockets
  in `/tmp/codex-daemon-<uid>/` whatever `TMPDIR` says (measured), where the owner's own
  Codex keeps its sockets too. A Codex launch's profile asks for a folder of the session's
  own shown at `/tmp/codex-daemon-<uid>` inside the sandbox (`FolderMount`), through a
  Greywall fork feature, proposed on the greywall list.
- **The mount source is the daemon's, outside everything the agent writes.** Greywall's
  mount refuses unsafe sources but checks before it mounts, so a source the agent could
  swap in between would mount any host folder. The source is
  `$XDG_RUNTIME_DIR/cockpit/mounts/<session>/codex-daemon` (`HostPaths::mount_sources`), not
  `<session dir>/codex-daemon`: no agent is granted it, its session folder, or any folder
  above them. `Profile::build` refuses a source whose folder is inside, or holds, a folder
  the agent writes (the checkout, the session's directory, a desktop socket).
- **Greywall's `--mount`, as landed in Theo's fork:** `--mount <host-dir>:<sandbox-path>`
  (repeatable, Linux only; settings key `filesystem.mounts`). The target must be below
  `/tmp` (not `/tmp` itself, no overlap, not `/tmp/greywall*`); the source must be an
  absolute, clean, existing directory the caller owns, not a link, the project, the home,
  greyproxy's data or `~/.config/greywall`. Cockpit detects it from `greywall --help`
  listing `--mount`, not from a version string, and passes it on the command line. Cockpit
  creates the source and its session folder owner-only before the launch and refuses a link
  or a folder of another user there. Greywall checks the source and then mounts it, so the
  source must also be out of the agent's reach (next point).
- **Without the feature, delivery is refused, not the launch.** The plan's `reach` is
  `ProviderReach::Unreachable` naming `--mount`. The agent runs with a private `/tmp`, and
  the daemon's `SessionTreeSockets` refuses to open its socket with that reason, connecting
  to nothing. On macOS greywall refuses a configuration with mounts, so a macOS launch is
  `Unreachable` even when `--help` lists the flag.
- **The daemon never follows Codex's link to the host.** With the folder mounted,
  `codex.sock` links to `/tmp/codex-daemon-<uid>/<entry>` as the sandbox sees it. The daemon
  reads that link as the source's `<entry>` (`FolderMount::host_path`), refuses a link
  anywhere else, and keeps #442's check that the listener is in the session's process tree.
  Codex's entries are 64-character names, too long for a socket address under the daemon's
  folders, so on Linux the daemon connects through the open folder
  (`/proc/self/fd/<folder>/<entry>`).
- **Ownership:** this lane owns the backend and the detection. The provider-wiring lane
  (#411) passes the launch plan's `reach` to `SessionTreeSockets` and wires Codex delivery
  through it.
- **Rejected:** sharing the host's `/tmp/codex-daemon-<uid>/` (#437 as written: the agent
  would reach the owner's unsandboxed Codex); reaching the socket through
  `/proc/<pid>/root` (Linux only, unmeasured with greywall's user namespace).
- **Not measured here:** a live run. The greywall installed here predates `--mount`, and the
  pinned commit does not have it; the tests use stand-in greywalls whose `--help` lists it or
  not. Re-pinning to the fork build with `--mount` and a live Codex delivery through it are
  #433's.
