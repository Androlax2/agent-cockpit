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
   readable id is answered with a `malformed-frame` frame. Messages built from a peer's input
   are capped at 256 bytes.
3. After `subscribe` (`from` a `snapshot` or a cursor), `changes` frames carry a
   `ChangeBatch`: events with entity, entity revision and an `upsert` or `tombstone`, ending
   at a cursor (database epoch plus commit revision). `follows` says whether the batch is a `snapshot` replacing the client's state
   or continues from a `cursor`; a cursor other than the client's own is a gap.
4. The agent channel (report 17): on the agent socket, `agent-report` carries an
   `AgentReport` and, from a newly launched agent, the `launch_token` it was given in
   `COCKPIT_LAUNCH_TOKEN`. No command names the sender's session; the daemon identifies it.
   The owner reads every session's latest report with `agent-reports`. A refusal the sender
   can act on carries `unblock`: the `action` that would lift it and who can (`owner`,
   `agent`, `cockpit`).
5. Approvals (#34): on the agent socket, `ask-run` and `ask-guide` file a request and
   `ask-run-status` / `ask-guide-status` read the sender's own; `ask-check` says whether an
   ask would reach the owner. The owner reads `approval-queue` and decides with
   `approval-approve`, which names the `revision` and `digest` it read, `approval-decline`,
   `approval-edit` (a new revision) and `approval-input`; guides with `guide-answer` and
   `guide-stuck` (naming the step shown) and `guide-close`. Views carry the display rules'
   `ShownText` beside the exact text, with the kinds escaped as `warnings`; a run request
   carries `folded_command`, `timeout_without_foreground` and, in the owner's views of an
   asking request, `changed_build_files` (#156). An approval within the daemon's grace after
   the request appeared or changed is refused with code `refused`.
6. Sessions (#31): `session-new` (an absolute `dir`, optional `agent`, `prompt` and
   `clone` branch), `session-list` (`ended` adds the ended ones), `session-restart`,
   `session-restart-all`, `session-sleep`, `session-wake`, `session-resume`, `session-end` and
   `session-link` answer a `session` view, `sessions`, or `restarted-all` with each
   session that could not restart; `session-end-finished` answers `ended-finished` with each
   session ended and each it could not end. A view's `finished` says the task list has the
   session's work done (#157). Owner socket only.
7. Tasks (#38): `task-add`, `task-list`, `task-show`, `task-start`, `task-done` and
   `task-delete` serve both sockets (an agent acts on its own project's items and what it
   filed); `task-propose` and `task-report-blocker` are the agent's; `task-edit`,
   `task-priority`, `task-queue`, `task-approve`, `task-decline`, `task-reopen`, `task-wait`,
   `task-unwait`, `task-image-add`, `task-image`, `task-draft-save`, `task-drafts` and
   `task-draft-discard` the owner's. A `TaskView` carries `next`, what has to happen for the
   item to move. Image bytes travel as lower-case hexadecimal text (`HexBytes`).
8. Frontends (#35): `provider-list` answers `providers` (each name `session-new` takes, and
   whether it resumes); `manager-open` opens or finds the manager tmux session (`column` and
   `viewer` words, from `dir`) and answers `manager` with the words that attach a terminal;
   `session-show` shows a session in the manager's viewer, waking it when asleep, and
   answers its `session` view. Owner socket only.

Forward compatibility: unknown object fields are ignored; capabilities, refusal reasons,
error codes and entity kinds keep unknown values in an `Unknown` variant. Change kinds are
closed on purpose: a client must not skip a change it cannot apply.

Framing: one JSON value per line, UTF-8, ended by `\n`, at most [`MAX_FRAME_BYTES`] including
the newline, in both directions (#21 chose it; #26 moved the limit here so the client and the
daemon share one constant).
