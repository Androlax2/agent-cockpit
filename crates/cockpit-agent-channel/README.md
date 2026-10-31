# cockpit-agent-channel

Owns the one authenticated channel between sandboxed agents and the daemon: launch tokens,
the verified process tree behind each sender and the per-project answers directories.

Report 17: an agent's request names no identity. The daemon reads the sender's process from
the kernel (the agent socket's peer credentials) and this crate decides which session that
process acts for:

1. The launcher calls `mint` for a session and passes the token to its agent
   (`COCKPIT_LAUNCH_TOKEN`), then `record_root` with the process it started.
2. The first request carrying that token binds the launch, but only from the recorded root
   or one of its descendants, and spends the token. A token presented from another tree is
   refused and stays unspent.
3. Every later request is identified by its ancestry alone: the nearest ancestor that is a
   bound launch's root names the session. Nested agents and hooks therefore act as the
   session that started them, with its grants.

`identify` takes a write transaction because step 2 writes. Requests that otherwise write
nothing (the approval status polls) use `identify_bound` in a read transaction: it gives
the same session or refusal, and `None` when a pending token needs `identify` to bind it.

A process is identified by boot id, pid and start time, so a reused pid never matches. The
walk stops at a parent that started after its child. Reading process trees is implemented
for Linux (`/proc`); elsewhere every sender is refused as unverifiable (report 11 keeps
macOS `validate`).

Every refusal carries an `Unblock`: what would lift it and who can (the owner, the agent,
or Cockpit itself). The capability that serves a verb identifies its sender here, then
decides with `cockpit-authorization`: agents' reports (state, label, doing-now, open
question) belong to `cockpit-sessions` since #146, approvals to `cockpit-approvals`. `todo`,
`task`, `evidence` and `message` are added by the issues that own those capabilities, with
this refusal shape.

`AnswersDirectory` creates `<runtime dir>/answers/<project>` private to the owner and writes
each answer once, atomically, read-only. The sandbox launcher mounts a project's directory
read-only into that project's sandboxes only.

## Dependency edges

- Depends on: `cockpit-turso` (transactions, migrations) and `cockpit-authorization` (every
  request is evaluated before it takes effect).
- Never on `cockpit-protocol` or a client: the daemon converts to wire types at its boundary
  (`cockpit-workspace-rules/tests/crate_graph.rs`).
- Used by: the daemon, which serves the agent socket.

## Tests

- `tests/identity.rs`: binding, spent and leaked tokens, pid reuse and nested sessions on a
  real state database with real processes.
- `tests/identify_bound.rs`: the read-only path beside a held writer, pending tokens left
  to `identify`, and the same refusals.
- `tests/answers.rs`: directory modes, refused names and write-once answers.
