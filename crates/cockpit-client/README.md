# cockpit-client

The typed client every frontend and agent uses to talk to the daemon over its Unix socket.

It owns the client side of the wire: connecting, the handshake, framing, requests with
deadlines and cancellation, and following the change feed with a cursor. It owns no state,
policy or rendering; frontends keep their own models and apply the batches it hands them
(report 02, report 03, report 04).

## Dependency edges

- Depends on: `cockpit-protocol` for every wire type, `tokio` for the socket and tasks,
  `serde` and `serde_json` for framing. Nothing else (`tests/dependencies.rs`).
- Used by: `cockpit-cli`, `cockpit-tui`, `cockpit-web`, `cockpit-notify` and agents' commands.
- Must never reach `cockpit-turso` or `cockpit-scheduling`, directly or through another crate
  (the `clients ->` rules of `cockpit-workspace-rules/tests/crate_graph.rs`).

## Contract

- **Framing.** One JSON value per line, UTF-8, ended by `\n`, at most [`MAX_FRAME_BYTES`]
  including the newline, in both directions. A larger frame, an unreadable frame or a frame
  cut by a disconnect closes the connection; it is never skipped.
- **Connect.** [`Connection::open`] sends a `Hello` with every version this build speaks and
  returns the daemon's `Welcome` or its typed refusal. The whole exchange has one deadline.
- **Requests.** [`Connection::request`] takes `&self`, so requests run concurrently on one
  connection and each reply reaches its own caller. Its timeout covers queueing, sending and
  waiting. A deadline or a dropped future only means the caller stopped waiting: the
  operation may still take effect (report 03, "cancellation has two facts"); a late reply is
  discarded and never reaches another request. A disconnect fails every request in flight
  with the reason.
- **Change feed.** A [`Subscription`] owns its own connection, the only one that asks for
  the `change-feed` capability; on a plain connection the daemon refuses `subscribe` as
  `not-negotiated`. Each [`PendingBatch`] must be applied completely, deletions
  included, before [`PendingBatch::applied`] advances the cursor; a batch dropped unapplied
  is returned again by the next call. A batch that follows a `snapshot` replaces all of the
  caller's state. After a disconnect, or a batch that does not follow the applied cursor (a
  gap), the call fails loud and the next call reconnects and subscribes from the applied
  cursor. When the daemon answers `resnapshot-required`, the subscription asks for a
  snapshot instead, never a silent jump.
- **Daemon ordering.** The daemon answers `subscribe` before it sends that subscription's
  first batch. The client reads ahead a bounded number of batches; batches sent before the
  reply could fill that buffer and stall the reply until the deadline.
