# The daemon holds an agent's request until its launch's process tree is recorded

Issue #279. Decided by the worker: the issue left the daemon hold and a CLI retry open.

- **The daemon waits, read-only and at most 5 s, while the request's launch token names a
  pending launch without a root** (`cockpit_agent_channel::awaits_root`, called from
  `Sender::of` in `crates/cockpit-daemon/src/approvals.rs`, which every agent route goes
  through). Identification then decides as before, so a launch that never records its root
  is still refused `LaunchNotStarted`. Every client is covered at once (the CLI, the Claude
  hook bridge, future bridges), and the refusal's "Cockpit unblocks it" stays true.
- **5 s, half the CLI's 10 s request deadline:** the launcher records the root moments after
  the agent starts (20 ms leader-file polls), and the agent must read the refusal, not a
  timeout, when the launch failed. The request holds its bulk-lane worker meanwhile.
- **Polling, not a notification from `cockpit-sessions`:** one indexed read every 20 ms,
  only for a request whose token is still pending, keeps the wait inside the daemon's
  agent routes without a new channel between capabilities.
- **Rejected:** a CLI retry on `LaunchNotStarted`, because the wire code
  (`UnidentifiedPeer`) is shared with refusals that must not be retried, and every bridge
  would need the same retry.
