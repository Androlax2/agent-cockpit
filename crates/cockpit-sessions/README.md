# cockpit-sessions

Agent sessions: provider conversations and their lifecycle (start, restart, sleep and wake, resume, end, reboot restore) with the session and launch identities.

A session is one provider conversation run by Cockpit in a checkout. Its id (`s` and 12 hex
digits) names it everywhere: its tmux session, its checkout occupancy, its identity on the
agent channel. Each launch of its agent is a row with its kind (`start`, `restart`, `wake`,
`resume`, `restore`) and the `cockpit-processes` execution it runs as, so the identity
survives every process exit. The state, launches and every transition live in the state
database; transitional states (`launching`, `stopping`) are stored too, so a daemon that
stopped in the middle of one finishes or repairs it at its next start.

## Rules

- **One start path.** `start` takes a directory of a registered project and runs the agent
  there, or in a new parallel clone on the given branch, which the session then occupies.
  A main checkout is shared by the sessions started in it.
- **Agent process.** The agent runs in a tmux pane on Cockpit's private server, as a hosted
  execution of `cockpit-processes`: its containment and whole-tree termination are the same
  as any execution's. It gets the daemon's base environment, its provider's variables,
  `COCKPIT_SESSION` and a one-use `COCKPIT_LAUNCH_TOKEN`, and only `TERM` and `COLORTERM`
  from tmux.
- **Restart, sleep, wake and resume** relaunch the same conversation, so each needs a
  provider with the `resume` capability that named the conversation; otherwise they are
  refused before anything stops.
- **Resume refuses a conversation alive elsewhere** (#157): while another session's agent
  of the same conversation is observed alive by `cockpit-processes`, or another session
  holding it is launching, running or stopping, with the session named in the reason.
- **Restart from an unknown status** (#157): a session an error or a crash left
  `launching` or `stopping` restarts like a running one; the stop observes and stops its
  agent first, so a restart is never a no-op. `start` holds the session's lock through its
  first launch.
- **Agents' reports.** An agent reports its state (working, waiting), label, doing-now,
  open question and how its last turn ended (#183) through the agent channel, which
  identifies the sender; this crate checks the fields, asks `cockpit-authorization` whether
  the session may change its own state, and keeps the latest report per session with the
  launch it came from (moved from `cockpit-agent-channel` in #146). Restart and sleep trust
  only a report of the current launch.
- **Start files.** Each launch records the state of the start files (`cockpit-config`) its
  provider reads, before the agent runs. A running session whose files changed since shows
  them in `session list`.
- **Restart-all** restarts only the running sessions whose start files changed: now when
  the agent's latest report since its launch says it waits at its prompt with no question,
  otherwise once it does (the periodic pass runs the deferred restart). Nothing restarts by
  itself (#177).
- **Wake paths.** `wake` is the owner's verb and refuses a session that is not asleep;
  `ensure_awake` is for callers that need the agent running (open, message, todo, phone)
  and leaves a running or launching session as it is. Both record the cause in the
  transition history.
- **Idle sleep.** The periodic pass puts a running session to sleep only when its agent's
  latest report, sent by the current launch's agent, says it waits at its prompt and asks
  nothing, and the idle window (60 minutes) has passed since that report. With no such
  report it is never put to sleep. The expiry counts from the move to asleep. A session asleep past the expiry (7 days) ends. Both values come from the current
  cockpit's sleep rules.
- **Self grant.** Each launch grants the session `change-state` over itself, in the
  transaction that mints its launch token, so its agent's reports are accepted; every move
  to `ended` or `failed` revokes it in the same transaction (`docs/decisions/2026-10-31-31-self-state-grant.md`).
- **Exits.** An agent observed gone ends its session with the reason; `resume` reopens it.
- **Stops are confirmed.** A stop whose termination `cockpit-processes` cannot confirm puts
  the session back to `running` with the reason and answers an error; it is never recorded
  as stopped. A stop an error left unfinished stays `stopping`, and `end` finishes it.
- **Failed launches clean up.** A launch that fails after its terminal opened closes it and
  stops its agent; `resume` stops the previous agent before it launches.
- **One operation per session at a time.** Owner operations and the periodic pass take the
  session's lock, and the pass re-reads the session under it.
- **Reboot restore** runs at daemon start, before requests are accepted. A session whose
  agent still runs is kept; a stop under way is finished; any other active session is
  reopened on its conversation. A reopen that fails, for any reason, leaves the session
  `failed` with the reason, and it is never retried silently: the owner resumes or ends it.
  Only a database error stops the daemon's start.
- **tmux may be missing.** The daemon still starts. Each operation that needs a terminal
  refuses, naming tmux, before it changes anything; restore and the periodic pass leave
  active sessions as they are until a daemon starts with tmux.
- **Design link.** One `http` or `https` URL per session.
- Nothing is imported from the current cockpit's state.

## Dependency edges

- Depends on: `cockpit-provider-api` (the only view of a provider), `cockpit-processes`
  (hosted executions, termination), `cockpit-tmux` (the agent's terminal),
  `cockpit-projects` (checkouts, clones, occupancy), `cockpit-agent-channel` (launch tokens,
  the agent's root identity, identifying a report's sender), `cockpit-authorization` (the
  session id the channel keys on, the self grant), `cockpit-config` (start files and their
  change detection), `cockpit-turso`, `tokio`, `tracing`.
- Migrations: version 2 moves the agent channel's reports, so the channel's migrations are
  applied first (the daemon's order).
- Must not depend on a provider adapter (`cockpit-claude`, `cockpit-codex`,
  `cockpit-opencode`): `sessions -> provider adapters` in the crate-graph rule.
- Used by: the daemon, whose `Routes` serve the `session` verbs to the owner and which runs
  the restore and the periodic pass.

## Tests

- `tests/schema.rs`: the stored state's constraints on a real database, and the id format.
- `tests/reports.rs`: report validation, merging, authorization and the migration that
  moved the reports here.
- `cockpit-daemon/tests/session_robustness.rs`: the resume refusal, restart from an unknown
  status, and the finished label the daemon computes from the task list (#157).
- `cockpit-daemon/tests/sessions.rs`: every lifecycle transition through the daemon's
  socket, against a fake provider with only the baseline contract, real processes and a
  private tmux server; `session_restarts.rs`: restart-all after a start file changed, and
  the wake paths.
- `cockpit-cli/tests/sessions.rs`: each `cockpit session` verb's request and output.
