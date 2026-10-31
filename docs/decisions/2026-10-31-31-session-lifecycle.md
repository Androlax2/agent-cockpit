# A session's agent is a hosted execution in a tmux pane, driven only through the baseline provider contract

Issue #31. Decided by the worker, from reports 04, 05 and 16 and the current cockpit's sleep
rules.

- **Hosted executions.** The agent needs a terminal (a tmux pane), and report 05 requires its
  termination to be observed by `cockpit-processes`. `Supervisor::prepare_hosted` builds the
  pane's command: it records the execution intent, clears the environment (`env -i`, keeping
  only `TERM` and `COLORTERM` from tmux; the values come from a 0600 file the step reads and
  deletes, never from argv, since the launch token is among them), enters the containment (systemd scope or process
  group) and writes the leader's pid and start token before it execs the agent.
  `confirm_hosted` waits for that file. tmux puts each pane in its own session, so the
  leader is the process group, and stopping a session is the same whole-tree termination as
  any execution (`crates/cockpit-processes/tests/hosted.rs`).
- **The baseline provider contract is its own crate.** `cockpit-provider-api` holds only what
  #31 needs: a provider's name, its `resume` capability, and the command for a new or resumed
  conversation with an optional first message. It has no dependency. Sessions never see an
  adapter (`sessions -> provider adapters` in the crate-graph rule). #33 extends the contract
  and adds the adapters.
- **Only clones are occupied.** A session started with `--clone` occupies its clone and
  releases it when it ends. A main checkout is shared by in-place sessions, as it is today.
- **Idle sleep needs the agent's word.** A session is put to sleep only when its agent's
  latest report on the agent channel, sent since the current launch, says `waiting`, with no
  open question, for the idle window (60 minutes from that report). Without a
  report it is never put to sleep. A launch grants the session the `change-state` over
  itself that reporting needs (`2026-10-31-31-self-state-grant.md`). An asleep session ends
  after 7 days. Both values are the current cockpit's defaults (`apps/crates/cockpit-core/src/sleep.rs`). The owner's manual
  `sleep` is refused only when the session is not running or could not be woken.
- **Restore is automatic and fails loud.** At daemon start, before the sockets are bound:
  - A session whose execution still runs and whose tmux session exists is kept.
  - A stop under way is finished.
  - Any other active session is reopened on its conversation (launch kind `restore`).
  - A reopen that fails leaves the session `failed` with the reason. The next start does not
    retry it; the owner resumes or ends it.
- **An unconfirmed stop is not a stop.** When termination cannot be confirmed, the session
  goes back to `running` with the reason and the request answers `conflict`.
- **The daemon starts without tmux.** Report 09 keeps diagnostics working when tmux is
  missing, so a tmux that cannot be run, or is too old, is logged at start. Every session
  operation that needs a terminal (new, restart, sleep, wake, resume, ending a running
  session) is refused with the reason (`refused`) before it changes anything. Restore and
  the periodic pass leave active sessions as they are: their agents may still run in a
  server this daemon cannot reach, and a daemon started with tmux restores them. Any other
  failure to set up the server stops the daemon's start.
- **Scopes get `--expand-environment=no`.** From systemd 258, `systemd-run --scope`
  rewrites `$` in the command it runs, so `$$` in the launch script became `$` on hosts
  with a user manager (CI's Ubuntu runner). The switch exists from 254, so it is passed
  where `systemd-run --version` is 254 or later; before 258 scopes never expanded. The fake
  `systemd-run` in `cockpit-processes`' tests behaves the same way by version.
- **Session ids** are `s` and 12 random hex digits: one plain word that is a valid tmux name,
  a checkout holder and an agent-channel session id.
- **Rejected:**
  - Running the agent as an ordinary execution and attaching tmux to its terminal. The
    process would have no terminal of its own.
  - Trusting tmux's `pane_pid` alone. Report 04 requires the boot id, process group and
    start token identity.
  - Sleeping on screen silence. Report 05 forbids classifying screens outside the adapters.
  - Importing the current cockpit's sessions. The issue requires a fresh start.
- **Not here:**
  - The adapters (#33) and the TUI views (#35).
  - Wake on open, message, todo or phone; the callers arrive with them.
  - Configuring the idle window from the owner configuration.
  - Capturing a stuck agent's screen for diagnostics.
