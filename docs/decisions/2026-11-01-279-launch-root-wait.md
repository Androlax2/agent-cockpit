# The daemon holds an agent's request until its launch's process tree is recorded

Issue #279. Decided by the worker: the issue left the daemon hold and a CLI retry open.

- **The daemon waits, read-only and at most 5 s, while the request's launch token names a
  pending launch without a root** (`cockpit_agent_channel::awaits_root`, called from
  `Sender::once_launched` in `crates/cockpit-daemon/src/approvals.rs`, which every agent
  route goes through). Identification then decides as before, so a launch that never records its root
  is still refused `LaunchNotStarted`. Every client is covered at once (the CLI, the Claude
  hook bridge, future bridges), and the refusal's "Cockpit unblocks it" stays true.
- **5 s, half the CLI's 10 s request deadline:** the launcher records the root moments after
  the agent starts (20 ms leader-file polls), and the agent must read the refusal, not a
  timeout, when the launch failed. The request holds its bulk-lane worker meanwhile, so only
  launches minted in the last 30 s are waited for (a failed launch's stray agent is refused
  at once), and neither is a sender whose process tree cannot be read or that has no token.
- **The wait wakes on the database's commit watch** (`Database::watch_commits`), since the
  root arrives with a commit, and needs no new channel between `cockpit-sessions` and the
  daemon's routes. Every request carrying a token pays one indexed read; a pending one reads
  again after each commit. #316 removes that read.
- **Rejected for now:** recording the root before the agent can run, which removes the race
  at its source but needs the hosted wrapper in `cockpit-processes` to wait for the daemon
  before it execs the agent: outside #279's files, filed as #316.
- **Rejected:** a CLI retry on `LaunchNotStarted`, because the wire code
  (`UnidentifiedPeer`) is shared with refusals that must not be retried, and every bridge
  would need the same retry.
