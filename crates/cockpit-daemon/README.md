# cockpit-daemon

Startup, wiring, request routing and supervised worker lifetimes; no business rules.

The `torlek-daemon` binary is this user's one daemon (reports 03 and 11). It holds no
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
  (which assesses the facts `readiness` gathers from these owners, #50), `cockpit-usage`
  (whose migrations it runs and whose plan windows its quota worker records, #58), `cockpit-tmux`
  (the private server sessions run in) and
  `cockpit-provider-api` (the providers it is wired with; the adapters arrive with #33);
  `tokio`, `serde`, `serde_json`, `tracing`.
- Used by: nothing. Frontends reach it only through `cockpit-client` over the socket; the
  `clients ->` rules of `cockpit-workspace-rules/tests/crate_graph.rs` keep them from
  linking it.

## Startup

1. Resolve the layout. The socket directory is `$XDG_RUNTIME_DIR/torlek`, or
   `$XDG_STATE_HOME/torlek/run` when that variable is unset (macOS). Both it and the state
   directory are created 0700; an existing one with any other mode or a symbolic link stops
   startup, it is never tightened.
2. Take the instance lock, `daemon.lock` in the socket directory. A second daemon of the same
   user exits 1 with `another torlek daemon of this user is already running`.
3. Start logging (`TORLEK_LOG_LEVEL`, report 12): `$XDG_STATE_HOME/torlek/logs/daemon`.
4. Write the agent instruction bundle under `$XDG_STATE_HOME/torlek/agent-context`
   (`cockpit-agent-context`), replacing what an earlier start wrote, and name it to every
   agent in `TORLEK_INSTRUCTIONS`. A bundle that cannot be written stops startup: agents
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
6. Open the private tmux server and bind `agent/agent.sock` (0600, stale file
   replaced). Serve agent reports, launch briefs, inbox polls and instructions while restoring
   the sessions (`cockpit-sessions`): each active
   session whose agent is gone is reopened, each failure logged and left `failed`. A tmux
   that cannot be run or is too old is logged instead, and the sessions are left as they
   are. The sessions' periodic pass (exits, idle sleep, expiry) starts as a supervised worker.
7. Once restore succeeds, bind `daemon.sock` (0600, stale file replaced), accept owner
   commands and other agent requests, start background dispatch and print `ready <socket>`.
   During restore, other agent requests receive `busy` with a retry instruction; owner clients
   cannot connect until restore finishes. A failed restore closes the early listener and its
   connections.

## Sockets (report 17)

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
  plan's typed `ProviderReach::Unreachable` reason. Owner `session-send` and `session-cancel`,
  queued work and coordinator messages use this connector through sessions (#431).
- OpenCode delivery receives its original launch's port and password from the host-owned
  sessions database, never from an agent-written endpoint file (#416). The same endpoint
  is retained when the daemon restarts; provider acceptance is not task completion.

- `askpass.sock` (#1374), in the private runtime directory beside `daemon.sock`, is where
  sudo's askpass helper in an approved run asks for the owner's password. The helper is
  `torlek-daemon --sudo-askpass <socket> <prompt>`; the daemon answers only peers running as
  the owner, and `cockpit-approvals` decides whether the helper's parent is the trusted
  sudo inside a live run. Once its program and output are checked, the daemon replies
  `seal` and the helper makes itself non-dumpable, refusing if it is traced already, before
  the owner is asked. The password goes back on the same connection, never into a log.

## Approvals (#34)

- Agents ask on the agent socket (`ask-run`, `ask-guide`, their status, `ask-check`); the
  owner decides on the host-only socket. The shell's syntax check and the working
  directory's resolution happen before the write transaction.
- What a request's `sudo` runs that its session can edit (#1383, `src/approvals/root_runs.rs`)
  is checked inside the write transaction that files, edits or (by a guide's `run:` step)
  files the revision, against the read-write folders of the session's staged launch spec,
  or as unsandboxed when there is none; the check reads files, at most 10 seconds. The
  owner's views read each recorded file again and bind their `digest` to what they read
  (`digest_as_read`); an approval is refused unless the files still read that way, and the
  run is not started when one no longer has its recorded hash. The rule scan (#1388) runs
  after it, against the request's project root as the clone, under its own 10 seconds, so
  a scan that does not end in time records a `cannot-tell` finding and leaves the sudo
  check's files pinned. The owner's views carry the findings with the display rules
  applied (`root_runs::findings_view`; a revision checked before the scan existed gets
  the sudo findings of its `root_runs`); the agent's do not. The files any part of the
  revision runs that the session can edit are pinned in the same transaction (#1387,
  `cockpit_approvals::pinned_files`, beside the sudo check under its own 10 seconds; past
  them, or for a file it could not record, a `cannot-tell` finding): the owner's views read each pinned file and each file sudo runs once, bind the
  `digest` to all of them, and an approval or a run is refused the same way when one
  changed.
- An approval commits the run intent, then the run starts outside the lanes: a long command
  holds no control worker. It is a `cockpit-processes` terminal execution (#163) of the
  daemon's one supervisor: a pseudo-terminal under `torlek-daemon --run-leader`
  (`cockpit_processes::run_session_leader`, which makes the terminal controlling without
  `unsafe`), in `$SHELL` when absolute or `/bin/sh`, with the daemon's environment minus
  `GIT_*` and the launch token, and `cockpit-projects`' `SAFE_CONFIG` as `GIT_CONFIG_*`.
  Processes the command leaves running keep its execution charged and are watched until
  they are observed gone.
- The run's terminal belongs to the daemon: a daemon that stops hangs the command up. One
  that ignores the hang-up is reattached at the next start; its exit code and later output
  are lost with the terminal.
- Approved runs are confined unless `agents.confined_runs = "off"` (#1387, `run_confinement`):
  hidden are the owner's keys, tokens, keyrings and browser profiles under their home
  (`HIDDEN_FROM_RUNS`), Torlek's token folders and the sessions' mount sources; emptied are
  Torlek's state, the owner's runtime folder and `/run`; reachable read-only, the agent
  socket's folder alone, never the owner's or the askpass socket. `ask-run` refuses a card
  folder inside what runs cannot see.
  A card's hosts go through `torlek-sandbox run-egress`, its folders under
  `<state>/run-egress`. `ask-run` resolves each card folder as it resolves the directory
  and checks each host as the proxy will read it (`cockpit_sandbox::card_entry`).
  Readiness names `confined runs`: off, available, or the reason every run is refused.
- The asking agent's project is the registered checkout holding its working directory,
  read from `/proc` after its process tree was verified, until sessions record it (#31).

## Coordinator kit (#410)

- On the agent socket, `agent-sessions`, `agent-message` and `agent-start` each identify the
  sender first and refuse without its coordinator grant, naming the grant and that only the
  owner gives it. The owner gives, revokes and reads them (`session-grant`, `session-revoke`,
  `session-grants`) on `daemon.sock` only.
- A message is audited and queued in the `cockpit-messaging` outbox in one transaction. A
  supervised loop (`coordinator-messages`) delivers it at the target's next prompt as a
  collaborator's message, while its sender still holds the grant and in the order sent, retrying while its agent cannot take input (a dialog, no report
  yet), for at most 24 hours. It uses the provider first, then terminal input only when
  delivery is unsupported (#431); an uncertain delivery is never retried as a fresh message.
- A target whose provider reports `inbox` (#703) collects its messages instead: its hook asks
  `agent-inbox`, which hands over the oldest one (producer `coordinator-inbox`) only while its
  agent waits at its prompt with no dialog, and records it `received` in the same
  transaction. A newer waiter supersedes an older one. `observe-sessions` shows each
  session's inbound message with why it waits, `stuck` after 60 s idle. Decided in
  `docs/decisions/2026-11-04-703-claude-message-inbox.md`.
- A message to an asleep session wakes it once queued, recorded `woken by a message`
  (#1326): only a sender the grant check let through wakes anything. The sender waits at
  most 5 s for the wake's outcome. A failed wake is logged and kept on a collected message
  while its session sleeps; a typed message's card says the session is asleep, as the
  delivery loop's next try records.
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
- A subscriber that reads slowly is owed a bounded buffer (#406): at most 8 batches and
  the owner's `memory.feed_buffer_kib` (1 MiB) of their wire lines wait for its connection.
  No batch is built while the buffer holds that much, so it holds at most one batch beyond
  it, and a batch larger than the whole buffer still passes. The feed waits for room, and
  the commits made meanwhile reach the subscriber in the next batch, each entity once, so
  nothing is skipped and nothing accumulates per commit.
- A supervised worker keeps the newest `storage.feed_history_changes` change rows
  (100,000 by default), hourly.
- Sessions and checkouts (#199): a supervised worker publishes their wire views
  (`session-list`, `checkout-list`) after every commit and every 5 s, and retracts ended
  sessions and removed checkouts. The daemon is the only producer of those kinds; a pass
  that derives what the last one wrote takes no write transaction, so it does not wake itself (`src/session_feed.rs`,
  `docs/decisions/2026-10-31-199-change-feed-projector.md`).

## Tasks (#38)

- `torlek todo` reaches `cockpit-tasks` on both sockets. An agent may file (`task-add`,
  `task-propose`, `task-report-blocker`), read and work items (`task-list`, `task-show`,
  `task-start`, `task-done`, `task-delete`); the rest is the owner's and refused on the agent
  socket. Proposals and blocker reports are refused on the owner's socket: the owner adds.
- The agent is identified as for its reports; its project is the registered checkout
  holding its working directory, as for approvals. Every project a request names (the
  target, the blocker project, a new item's draft) must be registered.
- Filings read the owner's `[tasks]` settings from the owner config at request time; a
  config that does not load refuses the filing instead of applying defaults.

## Workflows (#39)

- `torlek workflow` reaches `cockpit-workflows` on both sockets. Starting and cancelling a
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
  `/bin/sh`. Only a checkout the owner trusts runs checks, never a review checkout; anything
  else is refused before anything starts; a launch that may have started is recorded
  `unknown`.
- Where it runs follows `agents.sandbox` (#352, #862 Q1; `src/verification/environment.rs`).
  Under `"required"` a check runs through `torlek-sandbox` in a folder of its own beside the
  sessions' temporary folders, removed when it ends: the checkout writable, the daemon's
  agent base variables and its own `TMPDIR` only (never `GH_TOKEN`), its proxy allowing the
  checkout's ecosystem registries and the project's host grants (#862 Q2). A host its proxy
  refuses is a network request of the check run (`network.rs`), filed while it runs and
  again before its seal removes its folder. The run is usually over before the owner
  decides, so the request stands for the check: its next runs file no second one, it is
  allowed always (a grant the project's sessions use too) or denied, never once, and the
  run's output ends with a line naming it or its denial (`verification.rs`). The notifier
  tells it as the host's hold. One that declares
  `native` is recorded `unavailable`, never run on the host, and the gate's next action says
  to declare `sandbox` or turn the sandbox off. Under `"off"` every check but `docker` runs
  on the host with the approved commands' environment (Git settings included) and the risk
  is logged. A `docker` check names its `image` and is `unavailable` until the project's
  engine runs it (#856, #1392): no run reaches the engine before #390's checkout masks and
  container egress exist.
  A sandbox that cannot start is `unavailable` with its reason, and no process ran.
- The receipt records where the command ran (`ran in sandbox: <backend>`, or `native` with
  the sandbox off); a run that never started records nowhere.
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

## Project engines' egress (#390)

- A supervised worker (`src/engine_proxies.rs`) runs one `torlek-sandbox engine-proxy` per
  recorded engine whose user has an account, comparing the record with what runs every 2 s.
  Each proxy takes the listener its engine's relay hands it and serves the engine's pulls and
  containers from the owner's side. It runs with its own folder beside the sessions' mount
  sources (`engine-<project>`, owner only, its last proxy's requests, counters and decisions
  removed, so it allows nothing until the daemon writes the decisions as they stand),
  and with a pipe from the daemon as its standard input: a daemon that stops, even killed,
  closes the pipe, the proxy ends and the engine's egress port closes, so pulls and
  containers fail closed. An engine no longer recorded has its proxy ended; one that ends is
  started again after 10 s, and a changed engine's once its last proxy ended. A recorded
  engine no proxy can run for (its user has no account) is down with why; a proxy that
  keeps failing the same way is logged once.
- What a proxy refuses is a network request of the project's containers (`network.rs`): the
  owner allows it always, a project grant, or denies it, and the daemon writes the
  project's grants to the proxy's folder. Readiness shows `container egress` for a verified
  engine: available while its proxy serves, starting, or down with the proxy's last word.

## Scheduling (#43)

- A pass runs every `Wiring::scheduling` interval and on `queue-dispatch`, owner only. The
  daemon passes every session not ended as a candidate: its project from its checkout, its
  provider's capabilities, and what its current launch's agent last reported. The session
  last shown in the manager's viewer is the one the owner views.
- `queue-explain` reads the records the last pass wrote; nothing is computed on request.
- Each pass counts the agent seconds of every open item with a budget (`src/budgets.rs`,
  #412). When the turn recorder ends a turn of a session an item with a budget was placed on,
  and its sessions worked it for the budget or longer, it publishes a `budget-spent` entity,
  id the item's, in the turn's own transaction, so `torlek-notify` tells the owner. The same
  state again changes nothing; `task-budget`, `task-done` and `task-delete` retract it in
  their own transaction, so the next spent budget is news again.

## Database maintenance (#406)

A supervised task (`src/database_retention.rs`) checkpoints the WAL every
`storage.database_maintenance_interval_minutes`, takes a `VACUUM INTO` backup under
`<state>/backups` when the newest is older than `storage.database_backup_interval_hours` and
keeps `storage.database_backups_kept`, and, only with `storage.database_prune_automatic`,
runs the retention pass on the Bulk lane. The owner-only `storage-database` request reports
the pass, and with `apply` runs it (`docs/decisions/2026-11-04-406-database-retention.md`).

## Quota polling (#58)

A supervised task (`src/usage.rs`) reads the plan windows of each provider that reports
`quota`, first at the start, then every `usage.quota_refresh_minutes`, in the owner's home
(never a session's), off the async workers, each provider in a task of its own. Each reading, or why it failed, is recorded in
`cockpit-usage` under the account `default` until accounts arrive (#59). The alerts it fires
are logged and published in the reading's own transaction as `quota-alert` entities on the
change feed, id `provider/account/limit`, so `torlek-notify` tells each one and an alert is never
recorded as fired without reaching the feed. Each window's entity holds its last
alert until a reading shows the window below every threshold, which retracts it. A provider that fails waits twice as long as before each time, up to an hour, and
the refresh returns with its next reading. `Wiring::quota` is `None` in tests not about quota.

`usage-quota`, the owner's alone, answers every recorded account's windows judged at the
time asked under the worker's own policy, so `torlek usage quota` and the alerts agree,
each window with its queue pace under the daemon's own pacing (`window_pace`, #412). A
daemon whose wiring reads no quota refuses it rather than judge old readings by a default.

## Quota-stop resume (#158)

`quota_resume.rs` is a supervised worker (`quota-resume`) that, every session tick, sends each
open quota stop its one continue message: once a reading of the owner's own sign-in taken
after the stop shows a window spent and a later one shows room, or right after the #59
relaunch moved it. Coordinators go first; an asleep session is woken, then sent its message
once its agent waits. A stop no reading can judge becomes a needs-you dialog for the owner, as
does one whose #59 move to another account is refused, at once.
The agent channel's `report` counts a provider's transient-limit failures (`rate_limit` for
Claude Code), so the third within 30 minutes is recorded as an inferred quota stop, logged as
a warning, and relaunched and resumed like any other.

The cost archive (#699, `cost.rs`) is a second supervised worker. Every `Wiring::cost`
interval (a minute in the product; `None` in tests not about cost) it asks each provider
reporting `cost` for the replies past its cursors, in the owner's home (account `default`)
and in each live session's own home (the account of the session's launch), and archives
them in one transaction per home with the cursors. One scan runs at a time, each under a
deadline, so a home that hangs never stops the others. The sessions tell the worker before a
session's home is removed or the session relaunches on another account (`HomeObserver`), so
the replies the home still holds are archived under the account that paid for them; a
removal also drops the home's cursors, even when its last scan failed (those replies are
then lost, and logged). A scan whose thread outlives its 30 s deadline cannot be cancelled,
so no new scan starts while two such threads are still blocked. At start the worker prices what an earlier release
could not. `usage-cost`, the owner's alone, answers the archive's totals for the last 1 to
366 UTC days (`torlek usage cost`).

Time with the agent (#700): `turns.rs` is a supervised task (`turn-time`) that, after every
commit, opens a turn in `cockpit-usage` for each running session whose agent reports `working`
and ends it when the agent reports anything else or the session stops (the pass takes no write
when nothing changed, so it never wakes itself); `approvals.rs` counts each approval the owner
grants in the approval's own transaction. `usage-digest`, the owner's alone, answers each of the
last 1 to 366 UTC days (`digest.rs`: time with the agent, turns, approvals, cost, projects,
incidents) for `torlek usage digest`.

## Requests

- Peers are served only when the kernel's peer credentials name the daemon's own user.
- Framing is `cockpit-protocol`'s: one JSON line, at most `MAX_FRAME_BYTES`. A larger frame
  is answered `malformed-frame` and closes the connection; an unreadable one is answered and
  the connection stays open. A `hello` must arrive within the handshake deadline.
  The frame reader wipes every byte it read once it handed the byte on, and each frame once
  it is decoded: a frame may carry a secret the owner typed (#1386).
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

- `tests/usage_quota.rs`: issue #58: a fake provider's readings recorded under the owner's
  home, a failure recorded and retried twice as late each time, then the refresh again;
  `usage-quota` answering the windows and the last failure, and refused when no quota is read.
- `tests/cost_archive.rs`: issue #699: a synthetic week of Claude Code transcripts under the
  owner's home (repeated content blocks, a resumed conversation's replay, the UTC midnight, a
  fast-mode reply, a time-zone offset, a model with no price) archived to the nano-dollar
  against bills worked out by hand, later replies added without recounting, a restart not
  counting twice, a fake provider asked from its cursor, and a day count outside 1 to 366
  refused.
- `tests/usage_digest_turns.rs`: issue #700: a fake agent's `working` and `waiting` reports
  through a real session recorded as a turn under its project, a turn cut by the session's end,
  and one that survives a daemon restart; `tests/approvals.rs` counts a granted approval under
  its project in the digest and not a declined one.
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
- `tests/config_reload.rs`: issue #354: `torlek config reload` applies the owner's changed
  session settings to later launches, refuses an invalid file and keeps the settings in
  force, and is the owner's alone.
- `tests/hook_bridge.rs`: issue #203: `torlek provider list` shows Claude reports `events`
  through the hook bridge installed beside the daemon.
- `tests/agent_asks.rs`: issue #286: a daemon-launched agent's own `torlek ask run` and
  `torlek ask guide`, through the real CLI and agent socket, reach the owner's queue and
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
- `tests/memory_bounds.rs`: issue #406's feed buffer: a subscriber that stopped reading is
  owed its buffer, not a batch per commit; a batch larger than the buffer still passes; a
  written batch gives its room back. Its ignored test prints what stalled subscribers hold.
- `tests/sessions/no_agent.rs`: issue #406's session left running with no agent: reported
  to the owner after the window with the command that ends it, kept running until the
  owner ends it, not reported before the window nor while its agent runs, and left to the
  idle sleep when its agent waited at its prompt.
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
  roles, checks refused before they run (undeclared, untrusted), and the placement of each
  environment under `agents.sandbox` (#352), a `native` check under `"required"` ending with
  no process started.
- `tests/check_environment.rs`: issue #352 through the real sandbox: a check under
  `"required"` cannot read the owner's key or the daemon's state, reach the owner's socket,
  see `GH_TOKEN`, write the owner's cache or reach an ungranted host; its receipt names the
  sandbox and its folder is removed; a host it was refused reaches the owner as one request
  of the check, which its output and its reruns' name, a denial included; on a host without bubblewrap it is `unavailable` and the supervisor
  started no process; under `"off"` it runs on the host. `tests/live_enforcement.rs`
  (ignored, internet) has the check reach the host the owner allowed always and not another.
- `tests/scheduling.rs`: issue #43 on the binary's sockets: a queued item placed on the idle
  session of its project, the working session named in its explanation; `scheduling/budget.rs`,
  a turn ended past an item's budget put on the feed once and taken back by the owner's next
  decision (#412).
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
  the viewer, refused before the manager is open), `manager-fit` (the phone layout under
  `tui.phone_below_columns`: the column alone, or a three-row bar over a read-only viewer;
  back to side by side and read-write when wider, #855), `session-type` (a line typed into
  the session on screen, refused for any other), owner only.
