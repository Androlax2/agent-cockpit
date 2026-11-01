# A launched agent runs only once its launch's root is recorded

Issue #316. Decided by the worker, following the issue's direction; supersedes the wait of
`2026-11-01-279-launch-root-wait.md`.

- **The hosted launch script holds the program until a release file appears.** It writes the
  leader file, then waits for `<execution dir>/release` before it execs
  (`HOSTED_LAUNCH_SCRIPT`, `crates/cockpit-processes/src/platform.rs`).
  `cockpit-sessions` releases it in `confirm_agent` after `record_agent_root` committed
  (`Supervisor::release_hosted`), so no agent request can arrive before its root exists.
  Proof: `crates/cockpit-processes/tests/hosted.rs`
  `a_confirmed_hosted_program_runs_only_once_released`, and
  `crates/cockpit-daemon/tests/launch_root_order.rs`.
- **The daemon no longer waits for a root.** `awaits_root`, `ROOT_EXPECTED_WITHIN_SECONDS`,
  `LAUNCH_ROOT_WAIT` and the bulk-lane hold are gone, and so is the extra read per agent
  request; a request whose launch has no root is refused `LaunchNotStarted` (`unidentified-peer` on the wire) at once
  (`a_request_whose_launch_has_no_root_is_refused_at_once`).
- **A file, polled every 20 ms, not a FIFO:** a FIFO has no timeout in `sh`, and the release
  must bound the wait. Unreleased within `Supervisor::release_window` (the daemon sets
  the confirm's timeout plus twice the session step that records the root, 70 s), the script exits 126 without running the program, so
  a launch the daemon abandoned, or a daemon that died meanwhile, never starts its agent
  (`a_hosted_program_never_released_gives_up_without_running`). A discarded launch's pane is
  closed, which hangs the waiting script up sooner.
- **Every hosted launch is held,** not only agents': the product's one hosted caller is the
  sessions' agent, and one rule is easier to check than an opt-in.
- **Where the agent's identity cannot be read** (macOS), no root is recorded: the program
  is released all the same and the agent channel refuses it, as before.
