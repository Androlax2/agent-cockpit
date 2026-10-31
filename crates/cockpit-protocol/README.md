# cockpit-protocol

Versioned public wire types: handshake, commands, replies, errors and change-feed events.

It owns the published serialized form only: no database, UI, transport or policy. Capabilities
keep their own internal models and convert at the boundary, so internal changes never move
the wire. Report 14 treats this contract as stable from the first release.

## Dependency edges

- Depends on: `serde`, `serde_json`. No other Cockpit crate (`tests/dependencies.rs`).
- Used by: `cockpit-client`, `cockpit-daemon` and every frontend.

`apps/crates/cockpit-protocol` is the current programs' crate of the same name in the separate
`apps/` workspace; the two do not share code.

## Contract

1. The client sends a `Hello` listing every protocol version it speaks and the capabilities
   it wants. `negotiate` answers `accepted` with the highest shared version and the
   capabilities both offer, or `refused` with reason `version-mismatch` and the daemon's
   versions; `decode_hello` refuses an unreadable first frame with `malformed-hello`.
2. Then each `Request { id, command }` gets one `response` frame with the same id: `ok` with
   a `Reply`, or `error` with an `ErrorCode`. `decode_request` tells an unknown command
   (`unknown-command`) apart from a malformed one (`invalid-request`); a frame with no
   readable id is answered with a `malformed-frame` frame.
3. After `subscribe`, `changes` frames carry a `ChangeBatch`: events with entity, entity
   revision and an `upsert` or `tombstone`, ending at a cursor (database epoch plus commit
   revision). `follows` says whether the batch is a `snapshot` replacing the client's state
   or continues from a `cursor`; a cursor other than the client's own is a gap.

Forward compatibility: unknown object fields are ignored; capabilities, refusal reasons,
error codes and entity kinds keep unknown values in an `Unknown` variant. Change kinds are
closed on purpose: a client must not skip a change it cannot apply.

Framing (how frames are delimited on the socket and their size limit) is left to the client
and daemon issues (#21, #26).
