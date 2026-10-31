# Gate M0: WSL2 recorded run (checklist)

Open item of [the M0 gate proof](m0-foundation.md). No agent can run WSL2 from the Linux
sandbox, so Theo runs this on his Windows machine and records what happened. Copy this file
to `m0-wsl2-recorded-run-<date>.md`, fill in every blank, and link the copy from the proof.
A blank or skipped step is not a pass.

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

1. Clean-home start through the tests, as on Linux and macOS:

   ```sh
   cargo nextest run --locked --manifest-path Cargo.toml -p cockpit-daemon --test startup clean_home
   ```

   Expected: `2 tests run: 2 passed`. Result:

2. The rest of the gate's tests:

   ```sh
   cargo nextest run --locked --cargo-quiet --manifest-path Cargo.toml \
     -p cockpit-daemon -p cockpit-protocol -p cockpit-client -p cockpit-diagnostics \
     -p cockpit-workspace-rules \
     -E 'binary(startup) | binary(protocol) | binary(handshake) | binary(level) | binary(support_bundle) | binary(crate_graph)'
   ```

   Expected: every test passes. Summary line:

3. The real daemon on an empty home, with the WSL2 session's own `XDG_RUNTIME_DIR` (WSLg
   sets it to `/run/user/<uid>`; write down whether it was set):

   ```sh
   cargo build --locked --manifest-path Cargo.toml -p cockpit-daemon
   clean_home=$(mktemp -d)
   env -u XDG_STATE_HOME -u XDG_CONFIG_HOME HOME="$clean_home" target/debug/cockpit-daemon --foreground
   ```

   Expected: a line `ready <socket path>` on stdout, then the daemon keeps running.
   `XDG_RUNTIME_DIR` value: . Printed line: .

4. In a second WSL2 terminal, a second daemon for the same user refuses to start:

   ```sh
   env -u XDG_STATE_HOME -u XDG_CONFIG_HOME HOME="<the same clean_home>" target/debug/cockpit-daemon
   ```

   Expected: exit status 1 and `another cockpit daemon of this user is already running`.
   Result:

5. Interrupt the first daemon in its terminal and remove the temporary home (`rm -rf "$clean_home"`).

## Outcome

- All steps passed (yes/no):
- Failures, with the exact output:
