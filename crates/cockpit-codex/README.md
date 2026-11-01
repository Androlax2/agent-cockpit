# cockpit-codex

The Codex provider adapter: its launch words, capabilities pinned to a tested version, thread ids, and the in-sandbox hook bridge with launch-scoped hook trust.

Everything specific to Codex lives here; the daemon wires it in as a `cockpit-provider-api`
provider and nothing above the contract sees a Codex type (#48).

## Rules

- **Capabilities with provenance.** Resume and, with the hook bridge, events are `tested`
  only when the running Codex is `CONFORMANCE_VERSION`; on any other or unknown version they
  are `discovered` (unverified). Nothing else is reported: submit and cancel go through
  Codex's app-server, whose wiring waits on #407; outcome is not reported because a failed
  turn (an API error) sends no hook, so it would never end (measured on 0.161.0); effort,
  compact, billing, commands, evidence, remote control and screen reading are untested.
- **Launch.** `codex --no-daemon`, or `codex resume --no-daemon` with the thread id. Codex
  mints a new conversation's thread id itself, so a new launch names no conversation: the
  first report does (`--conversation`). The thread id and the first message come after `--`,
  where neither can be read as a flag. `--no-daemon` keeps the conversation out of Codex's
  shared background server, whose hooks would run with another launch's token (as #184 for
  Claude Code).
- **Hooks with launch-scoped trust.** Codex runs a hook only when its trusted hash is recorded.
  A launch passes `--enable hooks` and, with `--config`, each subscribed hook plus a
  `hooks.state` entry trusting exactly that hook's hash (`hooks::subscription`). Codex files
  them under its session-flags source, so they apply to this conversation only, never to a
  nested `codex` a tool starts, and the owner's persisted trust and the project's untrusted
  hooks stay as they are; `--dangerously-bypass-hook-trust` is never passed.
- **Hook bridge.** `cockpit-codex-hook <cockpit>` runs on `SessionStart`, `UserPromptSubmit`,
  `PostToolUse`, `PermissionRequest`, `Stop` and `Interrupt`, normalizes the payload
  (`hooks::normalize`) and reports through `cockpit agent report`, naming the thread it comes
  from (`--conversation`, the payload's `session_id`). A permission request reports a plain
  `--dialog` question, never the tool input; `Stop` and `Interrupt` report `waiting`. It
  prints nothing on standard output, so it never answers a permission request, and never
  exits 2. A subagent's event (`agent_id`) and a `SessionStart` after a compaction report
  nothing. `cockpit-codex-hook --version` answers its name and version for `Bridge::check`.
- **Known gap.** After `/clear`, Codex starts a new thread and sends no `SessionEnd` for the
  old one (measured), so the bridge cannot announce `--conversation-ends`, and the daemon
  refuses the new thread's reports (#408).

## Dependency edges

- Depends on: `cockpit-provider-api` only (crate-graph rule
  `provider adapters -> provider-api only`), `serde_json` for hook payloads, `sha2` for the
  hook trust hash.
- Used by: the daemon's wiring only (`capabilities -> provider adapters`).

## Tests

- `tests/launch.rs`, `tests/capabilities.rs`: the contract's words.
- `tests/hooks.rs`: conformance with the payloads and hook input schemas pinned in
  `tests/fixtures/codex-0.161.0/`, and the reports they become.
- `tests/trust.rs`: the subscription's `--config` values and the trust hash.
- `tests/bridge.rs`: the hook bridge as a process against a fake `cockpit`.
- `tests/bridge_check.rs`: `Bridge::check` passes the built bridge and refuses one that
  cannot run or is another program.
- `tests/discover.rs`: finding `codex` and its version.
- `tests/conformance.rs` (ignored without an installed Codex): the installed version is the
  pinned one, its help documents every flag a launch passes, it lists the `hooks` feature,
  Codex trusts every hook a launch subscribes, and a real turn's hooks report through the
  bridge.
