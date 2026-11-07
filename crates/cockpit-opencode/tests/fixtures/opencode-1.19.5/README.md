# OpenCode 1.19.5 events

One event per form the adapter tells apart, as Cockpit's plugin receives it in its `event`
hook: `{"id", "type", "properties"}` (OpenCode 1.19.5's `packages/opencode/src/plugin/index.ts`
hands each bus event to `hook.event({ event: { id, type, properties } })`).

## Captured native events (#417)

[`captured/events.jsonl`](captured/events.jsonl) contains **captured OpenCode 1.19.5 events**,
recorded on Linux x86-64 on 2026-11-02. The unmodified Cockpit plugin loaded through
`Provider::launch`'s `OPENCODE_CONFIG_CONTENT` under native Bun **1.4.4**, with `serverUrl`
matching the launch's private loopback port. A second observation-only plugin recorded
the runtime and URL; it did not generate events.

The native server emitted 19 forwarded events from three fixture turns: a completed `read`,
a `bash` permission request rejected through the authenticated owner API, and a held turn
aborted through that API. An unauthenticated permission reply returned HTTP 401. The
recorder passed each event unchanged into the built Rust bridge;
[`reports.jsonl`](captured/reports.jsonl) records its 19 actual `cockpit agent report`
argument lists. Abort reports waiting without a completed/failed outcome. The plugin's
brief appeared in all local model requests with tools.

This is native harness conformance with a Python standard-library loopback model and a
fake Cockpit command, not a signed-in model run or a TUI screenshot. Fresh HOME/XDG and a
cleared environment supplied no credentials. Model catalog fetching, updates, project
config, LSP, file watching and sharing were disabled. A read-only config directory makes
1.19.5's `Npm.install` return before installation; the test checks for unexpected package
files. No dependencies were installed and no account usage occurred.

Provenance:

- Official [Linux x64 release archive](https://github.com/anomalyco/opencode/releases/download/v1.19.5/opencode-linux-x64.tar.gz),
  explicitly approved by Theo: 79,651,896 bytes, SHA-256
  `588310a336fa3b9df36b3f0225b26f679d453efaad0cf3d66c3651ef3ec1647a`.
- Extracted binary SHA-256:
  `7df4e4136301f16589b0f30bf91a61380fca5f8ef22dd36570f5ebe1d4ec802c`.
  Its isolated `--version` and server health both returned `1.19.5`. Installed OpenCode
  `2.1.4` was not used as evidence for this pin.
- [`receipt.json`](captured/receipt.json) records the capture timestamp, runtime, source
  hashes, raw-record hashes and redactions. Only fixture roots, IDs, slug, timestamps and
  launch port changed; field shapes, ID relationships and event order remain intact.

Reproduce after obtaining the exact binary under the host's approval policy; the test
does not download it. Set `COCKPIT_OPENCODE_PROGRAM` to its absolute path and optionally
`COCKPIT_OPENCODE_CAPTURE_DIR` to a **new** task-owned directory under `.outgoing/`:

```sh
CARGO_BUILD_JOBS=2 cargo nextest run --locked -p cockpit-opencode \
  --test plugin_conformance -j 2 --run-ignored only --no-capture
```

`tests/events.rs` checks the captured semantics. `tests/bridge.rs` replays every captured
event through the real bridge and compares its report with the recorded report. The
observed shapes agree with the original fixtures on every field the adapter consumes;
the captured stream also retains native metadata those examples omitted. No adapter or
plugin behavior change was necessary.

The capture also exposes a pre-existing outcome-persistence bug, tracked in
[#516](https://github.com/Androlax2/agent-cockpit/issues/516): records 7–9 emit completed,
then busy, then idle before the next prompt. The adapter reports busy as working with
`--prompt`, which clears the stored completion in `cockpit-sessions`. These tests prove
native event capture and bridge reports; they do **not** prove that a completed outcome
remains visible to the owner. The original sequence is retained for that follow-up's
session-boundary regression and #51's conformance accounting.

## Schema-derived edge cases

The individual `.json` files directly in this directory remain **written from 1.19.5's
source**, originally for #47. They are not labelled captured. Their fields follow the
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

Question dialogs, child-session routing, retry, API errors, unfinished messages and the
deprecated `session.idle` remain schema-derived coverage, with Node plugin tests where
applicable. This capture does not claim native proof of those forms.

## Compaction events (#782)

`session-compaction-started.json`, `-ended.json` and `-failed.json` are **not captured and not
from 1.19.5**: a compaction needs a model reply, and 1.19.5 has no `session.compaction.*`
events. They follow OpenCode 2.2.5's event schemas, read as strings in its binary on
2026-11-07: `started` (`sessionID`, `reason` `auto|manual`, `inputID`), `ended` (`reason`,
`model`, `native`, `text`, `cost`, `tokens`) and `failed` (`reason`, `error`, `inputID`, `cost`,
`tokens`), with 2's `data` object where 1.19.5 has `properties`. The `tokens` object's shape
(`input`, `output`, `reasoning`, `cache.read`, `cache.write`) is OpenCode 1's assistant-message
shape, assumed to be the same, and the adapter reads the context before as the summary call's
input, cache read and cache write, and the context after as its output (report 686).

`cockpit-opencode`'s `CONFORMANCE_VERSION` names this directory; a new pinned version gets
a new directory and the tests move to it. `tests/conformance.rs` separately checks native
flags, variables, server authentication and command discovery.
