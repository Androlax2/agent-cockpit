# OpenCode 1.19.5 events

One event per form the adapter tells apart, as Cockpit's plugin receives it in its `event`
hook: `{"id", "type", "properties"}` (OpenCode 1.19.5's `packages/opencode/src/plugin/index.ts`
hands each bus event to `hook.event({ event: { id, type, properties } })`).

These are **written from 1.19.5's source**, not captured from a live run (#47): the session
that built the adapter could not run OpenCode. The fields and their names follow the
schemas at tag `v1.19.5` of `anomalyco/opencode`:

- `packages/schema/src/session-status-event.ts`: `session.status` (`idle`, `retry`, `busy`)
  and the deprecated `session.idle`.
- `packages/schema/src/v1/session.ts`: `session.created` (`sessionID`, `info` with
  `parentID` for a subagent), `message.updated` (an assistant message's `time.completed`,
  `finish`, `error` with its `name`), `message.part.updated` (a tool part's `state.status`).
- `packages/schema/src/v1/permission.ts`: `permission.asked`.
- `packages/schema/src/v1/question.ts`: `question.asked`.
- Identifiers have their prefixes (`ses_`, `msg_`, `prt_`, `per_`, `que_`, `evt_`) and
  lengths from `packages/schema/src/identifier.ts`; the values themselves are made up.

The ignored `tests/conformance.rs` runs the installed OpenCode; replacing these with
captured events is the open `validate` item of #47. `cockpit-opencode`'s
`CONFORMANCE_VERSION` names this directory; a new pinned version gets a new directory and
the tests move to it.
