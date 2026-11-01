# cockpit-daemon

Startup, wiring, request routing and supervised worker lifetimes; no business rules.

The `cockpit-daemon` binary is this user's one daemon (reports 03 and 11). It holds no
feature logic: capabilities own their rules and state, and the daemon starts them in order,
routes each request to a lane and keeps its workers alive. `apps/crates/cockpit-daemon` is
the current programs' crate of the same name in the separate `apps/` workspace; the two do
not share code.

## Dependency edges

- Depends on: `cockpit-protocol` (wire types and the frame limit), `cockpit-agent-context`
  (the instruction bundle it writes and serves, and each agent's launch brief, #49), `cockpit-config` (XDG
  layout), `cockpit-diagnostics` (its own log), `cockpit-turso` (the state database it opens
  once), `cockpit-authorization` (whose migrations it runs), `cockpit-agent-channel` (who an
  agent socket peer is, and its verbs), `cockpit-processes` (whose executions it reconciles),
  `cockpit-projects` (whose migrations it runs and whose verbs `Routes` serves to the
  owner), `cockpit-messaging` (whose migrations it runs, whose deliveries it reconciles and
  whose change feed `Routes` streams to the owner), `cockpit-approvals` (run requests and guides, and the runner of approved
  commands), `cockpit-sessions` (whose verbs `Routes` serves to the owner, and whose restore
  and periodic pass it runs), `cockpit-tasks` (whose verbs `Routes` serves to the owner and
  to agents), `cockpit-workflows` (whose verbs `Routes` serves to the owner and to agents,
  and whose migrations it runs), `cockpit-verification` (whose receipts it records from the
  check runs it executes, whose gate `Routes` serves to the owner and to agents, and whose
  migrations it runs), `cockpit-scheduling` (whose pass it runs every 30 seconds and on
  `queue-dispatch`, and whose explanations `Routes` serves to the owner), `cockpit-readiness`
  (which assesses the facts `readiness` gathers from these owners, #50), `cockpit-tmux`
  (the private server sessions run in) and
  `cockpit-provider-api` (the providers it is wired with; the adapters arrive with #33);
  `tokio`, `serde`, `serde_json`, `tracing`.
- Used by: nothing. Frontends reach it only through `cockpit-client` over the socket; the
  `clients ->` rules of `cockpit-workspace-rules/tests/crate_graph.rs` keep them from
  linking it.

## Startup

1. Resolve the layout. The socket directory is `$XDG_RUNTIME_DIR/cockpit`, or
   `$XDG_STATE_HOME/cockpit/run` when that variable is unset (macOS). Both it and the state
   directory are created 0700; an existing one with any other mode or a symbolic link stops
   startup, it is never tightened.
2. Take the instance lock, `daemon.lock` in the socket directory. A second daemon of the same
   user exits 1 with `another cockpit daemon of this user is already running`.
3. Start logging (`COCKPIT_LOG_LEVEL`, report 12): `$XDG_STATE_HOME/cockpit/logs/daemon`.
4. Write the agent instruction bundle under `$XDG_STATE_HOME/cockpit/agent-context`
   (`cockpit-agent-context`), replacing what an earlier start wrote, and name it to every
   agent in `COCKPIT_INSTRUCTIONS`. A bundle that cannot be written stops startup: agents
   would read another release's instructions.
5. Restart contract (report 03): open `state.db` with every capability's migrations
   (`MIGRATION_SETS`), then reconcile. `cockpit-processes` reattaches each execution it
   positively identifies, releases those observed gone and keeps the rest charged; each
   outcome is logged. New executions use a systemd user scope when this user's manager
   answers, otherwise a process group; the log names which. `cockpit-approvals` then ends
   the runs a stopped daemon left approved (`not-run`) or running, from what each one's
   execution did: never started (`not-run`), ended (`outcome-unknown`), or still running,
   which stays `running` and is recorded `outcome-unknown` once observed ended (#163).
   None is repeated. `cockpit-messaging` then turns every delivery a stopped daemon left in
   flight into a retry under the same key, when its receiver deduplicates, or into
   `uncertain`; each is logged.
6. Open the private tmux server, then restore the sessions (`cockpit-sessions`): each active
   session whose agent is gone is reopened, each failure logged and left `failed`. A tmux
   that cannot be run or is too old is logged instead, and the sessions are left as they
   are. The sessions' periodic pass (exits, idle sleep, expiry) starts as a supervised worker.
7. Only then bind `daemon.sock` and `agent/agent.sock` (0600, stale files replaced) and print
   `ready <socket>`.

## Two sockets (report 17)

- `daemon.sock` is host-only: no sandbox may reach it, and its peers are the owner.
- `agent/agent.sock` sits in a 0700 directory of its own, so a sandbox can be given that
  directory alone. Its peers are only ever agents: the kernel's peer process goes to
  `cockpit-agent-channel`, which decides the session from the launch token and the verified
  process tree. Owner commands there are refused `not-authorized`; `agent-report` on the
  host-only socket is refused.
- Each socket has its own connection limit. Agent requests run in the bulk lane, so
  sandboxed senders never take the owner's control capacity. They do share the bulk lane
  with the owner's bulk work: a flood of agent reports can make it answer `busy`.
- A provider's socket in the session's directory (Codex's `codex.sock`) is the agent's to
  replace, so the daemon never trusts its path: `SessionTreeSockets` opens it and keeps it
  only when the kernel's listening process descends from the session's agent root (#442).
  A link into a folder the sandbox mounts from the session's own (Codex's, #446) is read as
  that folder's entry, and delivery to a session whose sandbox could not mount it is refused with the
  plan's typed `ProviderReach::Unreachable` reason. Delivering through it is wired with #411.

## Approvals (#34)

- Agents ask on the agent socket (`ask-run`, `ask-guide`, their status, `ask-check`); the
  owner decides on the host-only socket. The shell's syntax check and the working
  directory's resolution happen before the write transaction.
- An approval commits the run intent, then the run starts outside the lanes: a long command
  holds no control worker. It is a `cockpit-processes` terminal execution (#163) of the
  daemon's one supervisor: a pseudo-terminal under `cockpit-daemon --run-leader`
  (`cockpit_processes::run_session_leader`, which makes the terminal controlling without
  `unsafe`), in `$SHELL` when absolute or `/bin/sh`, with the daemon's environment minus
  `GIT_*` and the launch token, and `cockpit-projects`' `SAFE_CONFIG` as `GIT_CONFIG_*`.
  Processes the command leaves running keep its execution charged and are watched until
  they are observed gone.
- The run's terminal belongs to the daemon: a daemon that stops hangs the command up. One
  that ignores the hang-up is reattached at the next start; its exit code and later output
  are lost with the terminal.
- The asking agent's project is the registered checkout holding its working directory,
  read from `/proc` after its process tree was verified, until sessions record it (#31).

## Coordinator kit (#410)

- On the agent socket, `agent-sessions`, `agent-message` and `agent-start` each identify the
  sender first and refuse without its coordinator grant, naming the grant and that only the
  owner gives it. The owner gives, revokes and reads them (`session-grant`, `session-revoke`,
  `session-grants`) on `daemon.sock` only.
- A message is audited and queued in the `cockpit-messaging` outbox in one transaction. A
  supervised loop (`coordinator-messages`) types it at the target's next prompt as a
  collaborator's message, while its sender still holds the grant and in the order sent, retrying while its agent cannot take input (a dialog, no report
  yet), for at most 24 hours. Provider-native delivery is #431.
- A start that needs confirmation joins the `ask run` queue; approving it starts the session
  and records it. One without confirmation starts at once, within the grant's hourly cap.
- Decided in `docs/decisions/2026-11-01-410-coordinator-kit.md`; proven end to end by
  `tests/coordinator_kit.rs`.

## Change feed (#41)

- `subscribe` is granted only on `daemon.sock`, to a peer that negotiated `change-feed`; the
  agent socket offers no capability. Its start (snapshot or replay) runs in the bulk lane;
  the answer is queued before the first batch, and forwarding the stream holds no lane place.
- One feed per connection: a second `subscribe` is refused `invalid-request`. Batches reach
  the writer on their own channel, so busy answer places never hold them back. A feed that
  stops (its history discarded, an error) or a batch larger than `MAX_FRAME_BYTES` closes
  the connection, so the client subscribes again from its cursor and gets
  `resnapshot-required` when that cursor cannot resume; nothing is skipped.
- A supervised worker keeps the newest 100,000 change rows, hourly.
- Sessions and checkouts (#199): a supervised worker publishes their wire views
  (`session-list`, `checkout-list`) after every commit and every 5 s, and retracts ended
  sessions and removed checkouts. The daemon is the only producer of those kinds; a pass
  that derives what the last one wrote takes no write transaction, so it does not wake itself (`src/session_feed.rs`,
  `docs/decisions/2026-10-31-199-change-feed-projector.md`).

## Tasks (#38)

- `cockpit todo` reaches `cockpit-tasks` on both sockets. An agent may file (`task-add`,
  `task-propose`, `task-report-blocker`), read and work items (`task-list`, `task-show`,
  `task-start`, `task-done`, `task-delete`); the rest is the owner's and refused on the agent
  socket. Proposals and blocker reports are refused on the owner's socket: the owner adds.
- The agent is identified as for its reports; its project is the registered checkout
  holding its working directory, as for approvals. Every project a request names (the
  target, the blocker project, a new item's draft) must be registered.
- Filings read the owner's `[tasks]` settings from the owner config at request time; a
  config that does not load refuses the filing instead of applying defaults.

## Workflows (#39)

- `cockpit workflow` reaches `cockpit-workflows` on both sockets. Starting and cancelling a
  workflow are the owner's; beginning a step and recording its checkpoint are an agent's.
  Reading, completing, releasing, blocking and unblocking serve both, and
  `cockpit-workflows` decides who may act on which step.
- `workflow-start` without a preset reads the item's project's `workflow.preset` from its
  config at request time; a config that does not load, or a preset no preset has, refuses
  the start instead of applying the default.
- `workflow-begin` checks the step's declared capabilities against what the session's
  provider reports, from the session record and the providers the daemon is wired with.
- No request can claim a requirement passed: completions carry the owner's waivers only,
  until `cockpit-verification` reports what passed (#40).

## Verification (#313)

- `check-run` and `check-receipt` are the owner's: the daemon runs a check the project
  declares under `[[checks]]` in its checkout, as an execution of its one supervisor, under
  `/bin/sh` with the approved commands' environment (Git settings included). Only a checkout
  the owner trusts runs checks, never a review checkout; only `native` checks run. Anything
  else is refused before anything starts; a launch that may have started is recorded
  `unknown`.
- Before the run starts, the daemon fingerprints the check's artifacts and reads the
  checkout's head and uncommitted changes with its own Git (`Git::source_state`). The run id
  is `execution-<id>`. When the program exits, the receipt is recorded with the exit code and
  the end of its output; nothing a request carries is recorded as evidence.
- `check-claim` is an agent's: its statement that a check passed, about the checkout it
  works in, recorded as a claim once the session holds `change-state` over itself. The gate
  shows a check with only claims as `unverified`.
- `check-gate` serves both: the owner names the project (and a checkout), an agent gets the
  checkout it works in. Evidence is read on a read transaction; the checkout is read after
  it ends.
- A daemon that stops while a check runs records nothing for it (#351).

## Scheduling (#43)

- A pass runs every `Wiring::scheduling` interval and on `queue-dispatch`, owner only. The
  daemon passes every session not ended as a candidate: its project from its checkout, its
  provider's capabilities, and what its current launch's agent last reported. The session
  last shown in the manager's viewer is the one the owner views.
- `queue-explain` reads the records the last pass wrote; nothing is computed on request.

## Requests

- Peers are served only when the kernel's peer credentials name the daemon's own user.
- Framing is `cockpit-protocol`'s: one JSON line, at most `MAX_FRAME_BYTES`. A larger frame
  is answered `malformed-frame` and closes the connection; an unreadable one is answered and
  the connection stays open. A `hello` must arrive within the handshake deadline.
- A [`Handler`] routes each command to an immediate answer or to a lane. `control` (input,
  status, stop, approval) and `bulk` have separate workers and separate bounded queues, so
  saturated bulk work never takes control capacity. A full lane answers `busy` at once.
- A connection keeps at most `requests_per_connection` answers pending; past that the daemon
  stops reading it, so a client that does not read its answers cannot grow memory.
- Each job runs in its own task: a panic answers that request `internal` and nothing else.
  Started jobs are never aborted; a lane's places are released only when the job stopped,
  never when the client stopped waiting. Blocking work uses `spawn_blocking` inside the job.
- The accept loop is a supervised worker: a panic is logged and the loop restarted.

## The binary and the tests' daemon

`src/main.rs` reads the arguments and calls [`serve()`], which loads the owner's
configuration, finds Claude Code and its hook bridge in the installation's directory, and runs
the daemon. `examples/cockpit-e2e-daemon.rs` calls the same [`serve()`] with a shorter idle
window and session tick, so the TUI's end-to-end tests (#300) see a session fall asleep in
seconds without lowering the configuration's 30-minute floor. It is an example, so Cargo
builds it only when asked and no release installs it.

## Tests

- `tests/startup.rs`: the binary on a clean home, the second daemon, private modes, the
  macOS layout, a stale socket after `kill -9`, reconcile before accept.
- `tests/coordinator_kit.rs`: issue #410's acceptance with the fake provider: every
  coordinator verb refused without its grant, then a coordinator that sees a worker's
  question, messages it and starts a worker the owner approves, audited with both sessions.
- `tests/protocol.rs`: handshake, version refusal, framing and `not-negotiated`, through the
  binary's socket.
- `tests/agent_channel.rs`: issue #32's acceptance on the binary's sockets: a launched
  agent's report, leaked tokens, payload-claimed sessions, a nested agent with its parent's
  grants, refusals with their unblock, restart.
- `tests/launch_root_order.rs`: issue #316: launched agents' first reports land without a
  retry, and a request whose launch has no root is refused at once, never held.
- `tests/config_reload.rs`: issue #354: `cockpit config reload` applies the owner's changed
  session settings to later launches, refuses an invalid file and keeps the settings in
  force, and is the owner's alone.
- `tests/hook_bridge.rs`: issue #203: `cockpit provider list` shows Claude reports `events`
  through the hook bridge installed beside the daemon.
- `tests/agent_asks.rs`: issue #286: a daemon-launched agent's own `cockpit ask run` and
  `cockpit ask guide`, through the real CLI and agent socket, reach the owner's queue and
  get the owner's answer back as their exit code.
- `tests/approvals.rs`: issue #34's acceptance on the binary's sockets: a command run in a
  terminal with its exit code, the display rules in the queue, redaction, Git hooks off,
  report 04's walkthrough 3, decline, input to a waiting command, the syntax check, grants,
  `ask-check`, socket roles and a guide; #163's restarts: a run a crash hung up is reported
  ended and never repeated, one that outlives the restart is reattached until it ends.
- `tests/change_feed.rs`: issue #41's feed through the socket: snapshot then each commit,
  resume after a restart with tombstones, `resnapshot-required` for another epoch and for
  discarded history, one feed per connection, none on the agent socket, batches with one
  answer place, an oversized snapshot closing the connection.
- `tests/session_feed.rs`: issue #199's sessions and checkouts reaching a subscriber as
  they start, sleep and end, each in the exact state the command answered; a foreign
  `session` entity retracted; a broken owner configuration holding back checkouts only.
- `tests/tasks.rs`: issue #38's agent side on the binary's sockets: a proposal approved,
  the cross-project cap, blocker reports and their configuration, a summary read by the item
  waiting on it, and socket roles.
- `tests/workflows.rs`: issue #39 on the binary's sockets: a restart mid-step resuming at
  the checkpoint, the project's preset and the owner's, socket roles, a sender without a
  session record refused a step, and requirements met only by the owner's waiver.
- `tests/verification.rs`: issue #313 on the binary's sockets: a run yields the receipt the
  gate counts, a failure is recorded failed with its output, a change after the run is stale,
  an agent cannot run a check or read a receipt, an agent's claim stays unverified, socket
  roles, and checks refused before they run (undeclared, untrusted, another environment).
- `tests/scheduling.rs`: issue #43 on the binary's sockets: a queued item placed on the idle
  session of its project, the working session named in its explanation.
- `tests/executions.rs`: a daemon started after a killed one reattaches an execution that
  kept running and releases one that ended meanwhile, before it accepts requests.
- `tests/overload.rs`: issue #26's overload fixture and panic containment, on the real socket
  and lanes with a fixture [`Handler`] that blocks or panics on `subscribe` (#108).
- `tests/sessions.rs`: issue #31's lifecycle through the socket with a fake provider: start
  in place and in a clone, restart and restart-all, sleep and wake, idle sleep, expiry, an
  exited agent and resume, end, reboot restore and its failure, design links.
- `tests/manager.rs`: issue #35's frontends verbs: `provider-list`, `manager-open` (the
  manager tmux session with its column and viewer, opened once), `manager-focus` (the column
  takes the keys from the viewer, #317, and gives them back, answering whether the viewer
  had them, #345), `session-show` (waking an asleep session into
  the viewer, refused before the manager is open), owner only.
