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
  once) and `cockpit-authorization` (whose migrations it runs); `tokio`, `serde`,
  `serde_json`, `tracing`.
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
4. Restart contract (report 03): open `state.db` with every capability's migrations, then
   reconcile; no capability holds launch intents or executions yet.
5. Only then bind `daemon.sock` (0600, a stale file replaced) and print `ready <socket>`.

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
- `tests/overload.rs`: issue #26's overload fixture and panic containment, on the real socket
  and lanes with a fixture [`Handler`] that blocks or panics on `subscribe` (#108).
