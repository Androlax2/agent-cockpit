# Gate M0: WSL2 recorded run (checklist)

Fallback for [the M0 gate proof](m0-foundation.md), whose WSL2 item is the `greenfield-wsl2`
CI job. Use this for a hand run on a real Windows machine, for example when the CI job and
a user's machine disagree. Copy this file to `m0-wsl2-recorded-run-<date>.md`, fill in every
blank, and link the copy from the proof. A blank or skipped step is not a pass.

## Machine

- Date:
- Windows version (`winver`):
- WSL version (`wsl --version`, in PowerShell):
- Distribution and release (`cat /etc/os-release`):
- Kernel (`uname -srm`):
- CPU and RAM:
- Repository commit (`git rev-parse HEAD`):
- `cargo --version` and `cargo nextest --version`:

## Steps

Run inside the WSL2 distribution, in a clone of the repository on the Linux file system
(`~/...`, not `/mnt/c/...`), with Rust 1.99.1 and cargo-nextest installed.

1. The gate's tests, the daemon's clean-home start included:

   ```sh
   cargo nextest run --locked --cargo-quiet --manifest-path Cargo.toml \
     -p cockpit-daemon -p cockpit-protocol -p cockpit-client -p cockpit-diagnostics \
     -p cockpit-workspace-rules \
     -E 'binary(startup) | binary(protocol) | binary(handshake) | binary(level) | binary(support_bundle) | binary(crate_graph)'
   ```

   Expected: every test passes. Summary line:

2. The real daemon on an empty home. Every XDG variable is unset, so the lock and socket
   stay inside the temporary home, apart from any other daemon in the WSL2 session. Write
   down the session's own value first (`echo $XDG_RUNTIME_DIR`; WSLg sets
   `/mnt/wslg/runtime-dir` without systemd, `/run/user/<uid>` with it):

   ```sh
   cargo build --locked --manifest-path Cargo.toml -p cockpit-daemon
   clean_home=$(mktemp -d)
   env -u XDG_RUNTIME_DIR -u XDG_STATE_HOME -u XDG_CONFIG_HOME HOME="$clean_home" target/debug/cockpit-daemon --foreground
   ```

   Expected: `ready <clean_home>/.local/state/cockpit/run/daemon.sock` on stdout, then the
   daemon keeps running. Session's `XDG_RUNTIME_DIR`: . Printed line: .

3. In a second WSL2 terminal, a second daemon for the same user refuses to start:

   ```sh
   env -u XDG_RUNTIME_DIR -u XDG_STATE_HOME -u XDG_CONFIG_HOME HOME="<the same clean_home>" target/debug/cockpit-daemon
   ```

   Expected: exit status 1 and `another cockpit daemon of this user is already running`.
   Result:

4. Interrupt the first daemon in its terminal and remove the temporary home (`rm -rf "$clean_home"`).

## Outcome

- All steps passed (yes/no):
- Failures, with the exact output:
