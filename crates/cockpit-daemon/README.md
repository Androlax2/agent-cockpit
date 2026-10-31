# cockpit-daemon

Startup, wiring, request routing and supervised worker lifetimes; no business rules.

The `cockpit-daemon` binary is this user's one daemon (reports 03 and 11). It holds no
feature logic: capabilities own their rules and state, and the daemon starts them in order,
routes each request to a lane and keeps its workers alive. `apps/crates/cockpit-daemon` is
the current programs' crate of the same name in the separate `apps/` workspace; the two do
not share code.

## Dependency edges

- Depends on: `cockpit-protocol` (wire types and the frame limit), `cockpit-config` (XDG
  layout), `cockpit-diagnostics` (its own log), `cockpit-turso` (the state database it opens
  once), `cockpit-authorization` (whose migrations it runs), `cockpit-agent-channel` (who an
  agent socket peer is, and its verbs), `cockpit-processes` (whose executions it reconciles),
  `cockpit-projects` (whose migrations it runs and whose verbs `Routes` serves to the
  owner), `cockpit-messaging` (whose migrations it runs, whose deliveries it reconciles and
  whose change feed `Routes` streams to the owner), `cockpit-approvals` (run requests and guides, and the runner of approved
  commands), `cockpit-sessions` (whose verbs `Routes` serves to the owner, and whose restore
  and periodic pass it runs), `cockpit-tmux` (the private server sessions run in) and
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
4. Restart contract (report 03): open `state.db` with every capability's migrations
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
5. Open the private tmux server, then restore the sessions (`cockpit-sessions`): each active
   session whose agent is gone is reopened, each failure logged and left `failed`. A tmux
   that cannot be run or is too old is logged instead, and the sessions are left as they
   are. The sessions' periodic pass (exits, idle sleep, expiry) starts as a supervised worker.
6. Only then bind `daemon.sock` and `agent/agent.sock` (0600, stale files replaced) and print
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

## Change feed (#41)

- `subscribe` is granted only on `daemon.sock`, to a peer that negotiated `change-feed`; the
  agent socket offers no capability. Its start (snapshot or replay) runs in the bulk lane;
  the answer is queued before the first batch, and forwarding the stream holds no lane place.
- One feed per connection: a second `subscribe` is refused `invalid-request`. A feed that
  stops (its history discarded, an error) closes the connection, so the client subscribes
  again from its cursor and gets `resnapshot-required` when that cursor cannot resume.
- A supervised worker keeps the newest 100,000 change rows, hourly.

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

## Tests

- `tests/startup.rs`: the binary on a clean home, the second daemon, private modes, the
  macOS layout, a stale socket after `kill -9`, reconcile before accept.
- `tests/protocol.rs`: handshake, version refusal, framing and `not-negotiated`, through the
  binary's socket.
- `tests/agent_channel.rs`: issue #32's acceptance on the binary's sockets: a launched
  agent's report, leaked tokens, payload-claimed sessions, a nested agent with its parent's
  grants, refusals with their unblock, restart.
- `tests/approvals.rs`: issue #34's acceptance on the binary's sockets: a command run in a
  terminal with its exit code, the display rules in the queue, redaction, Git hooks off,
  report 04's walkthrough 3, decline, input to a waiting command, the syntax check, grants,
  `ask-check`, socket roles and a guide; #163's restarts: a run a crash hung up is reported
  ended and never repeated, one that outlives the restart is reattached until it ends.
- `tests/change_feed.rs`: issue #41's feed through the socket: snapshot then each commit,
  resume after a restart with tombstones, `resnapshot-required` for another epoch and for
  discarded history, one feed per connection, none on the agent socket.
- `tests/executions.rs`: a daemon started after a killed one reattaches an execution that
  kept running and releases one that ended meanwhile, before it accepts requests.
- `tests/overload.rs`: issue #26's overload fixture and panic containment, on the real socket
  and lanes with a fixture [`Handler`] that blocks or panics on `subscribe` (#108).
- `tests/sessions.rs`: issue #31's lifecycle through the socket with a fake provider: start
  in place and in a clone, restart and restart-all, sleep and wake, idle sleep, expiry, an
  exited agent and resume, end, reboot restore and its failure, design links.
