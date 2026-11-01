# cockpit-codex

The Codex provider adapter: its launch words, capabilities pinned to a tested version, thread ids, the in-sandbox hook bridge with launch-scoped hook trust, and delivery through a private app-server per session.

Everything specific to Codex lives here; the daemon wires it in as a `cockpit-provider-api`
provider and nothing above the contract sees a Codex type (#48).

## Rules

- **Capabilities with provenance.** Resume and, with the bridge, events, submit, cancel and
  outcome are `tested` only when the running Codex is `CONFORMANCE_VERSION`; on any other or
  unknown version they are `discovered` (unverified). Effort, compact, billing, commands,
  evidence, remote control and screen reading are untested and not reported.
- **Launch with the bridge** (#407). The session runs `cockpit-codex-session`, which starts
  the session's private app-server, `codex app-server --listen unix://<session dir>/codex.sock
  --enable hooks --config <hook>…` (`server_arguments`), then the TUI against it, `codex
  [resume] --remote unix://<socket> [-- [<thread id>] [<first message>]]` (`tui_arguments`).
  Both run in the session's terminal with the launch's environment, so hooks keep the launch
  token; the server's working directory is the conversation's. When the TUI exits, the
  launcher stops the server (`SIGTERM`, which Codex's npm wrapper passes on, then `SIGKILL`
  to the wrapper and its descendants) and exits with the TUI's status. The server's output goes to `codex-app-server.log` beside
  the socket. A private server per session, never Codex's shared one: the shared server
  runs hooks with whichever launch's token started it (as #184 for Claude Code).
- **Launch without the bridge.** `codex [resume] --no-daemon`, with the thread id and the
  first message after `--`, where neither can be read as a flag; only resume is reported.
- **Thread ids.** Codex mints a new conversation's thread id itself, so a new launch names no
  conversation: the first report does (`--conversation`).
- **Hooks with launch-scoped trust.** Codex runs a hook only when its trusted hash is recorded.
  The app-server gets `--enable hooks` and, with `--config`, each subscribed hook plus a
  `hooks.state` entry trusting exactly that hook's hash (`hooks::subscription`). Codex files
  them under its session-flags source, so they apply to this session's server only, never to
  a nested `codex` a tool starts, and the owner's persisted trust and the project's untrusted
  hooks stay as they are; `--dangerously-bypass-hook-trust` is never passed.
- **Hook bridge.** `cockpit-codex-hook <cockpit>` runs on `SessionStart`, `UserPromptSubmit`,
  `PostToolUse`, `PermissionRequest`, `Stop` and `Interrupt`, normalizes the payload
  (`hooks::normalize`) and reports through `cockpit agent report`, naming the thread it comes
  from (`--conversation`, the payload's `session_id`). A permission request reports a plain
  `--dialog` question, never the tool input; `Stop` reports `waiting --outcome completed`,
  `Interrupt` reports `waiting`. It prints nothing on standard output, so it never answers a
  permission request, and never exits 2. A subagent's (`agent_id`) activity and a
  `SessionStart` after a compaction report nothing; a subagent's permission request still
  reports `--dialog`. `cockpit-codex-hook --version` and `cockpit-codex-session --version`
  answer their name and version for `Bridge::check`.
- **Failed turns** (`turns`). A failed turn sends no hook. It leaves its thread
  `systemError`, which the app-server announces to every connection; the launcher's watcher
  then reads the thread's last turn and reports it once, `waiting --outcome failed
  --outcome-reason <Codex's error kind>`, or `--outcome quota-stopped` for
  `usageLimitExceeded`. A helper
  thread's (ephemeral) or a subagent's failure is not the conversation's and is not reported.
- **Delivery** (`delivery`, `Provider::deliver`). Over the session's socket (WebSocket
  JSON-RPC, `app_server`): a message starts a turn (`turn/start`) when the thread is not
  `active`, or joins the turn under way (`turn/steer` with its id), and starts a turn when
  that turn ended meanwhile; a cancel interrupts the turn under way (`turn/interrupt`) and is
  refused when none is. The conversation is named by its thread id: the TUI also loads helper
  threads. Codex refuses to list a thread's turns for a moment after its first message, so a
  refused listing is retried for 2 seconds.
- **Known gaps.** After `/clear`, Codex starts a new thread and sends no `SessionEnd` for the
  old one (measured), so the bridge cannot announce `--conversation-ends`, and the daemon
  refuses the new thread's reports (#408). The socket's real file is Codex's, under
  `/tmp/codex-daemon-<uid>/`, which the session's sandbox and the daemon must share; the
  sandbox grants neither yet (#437). Where the approval requests of a delivered turn go is
  unmeasured (#436).

## Dependency edges

- Depends on: `cockpit-provider-api` only (crate-graph rule
  `provider adapters -> provider-api only`), `serde_json` for hook payloads and JSON-RPC,
  `sha2` for the hook trust hash, `rustix` for the launcher's `SIGTERM`.
- Used by: the daemon's wiring only (`capabilities -> provider adapters`), which comes with
  #411 together with shipping `cockpit-codex-hook` and `cockpit-codex-session` in the release.

## Tests

- `tests/launch.rs`, `tests/capabilities.rs`: the contract's words, and the app-server's and
  TUI's.
- `tests/hooks.rs`: conformance with the payloads and hook input schemas pinned in
  `tests/fixtures/codex-0.161.0/`, and the reports they become.
- `tests/trust.rs`: the subscription's `--config` values and the trust hash.
- `tests/bridge.rs`: the hook bridge as a process against a fake `cockpit`.
- `tests/bridge_check.rs`: `Bridge::check` passes the built programs and refuses one that
  cannot run or is another program.
- `tests/delivery.rs`: submit, steer and cancel against a fake app-server
  (`tests/common/app_server.rs`).
- `tests/turns.rs`: a failed turn's outcome and report.
- `tests/session.rs`: the session launcher as a process with a fake Codex: the server and TUI
  words, failed-turn reports, the server stopped with the TUI.
- `tests/discover.rs`: finding `codex` and its version.
- `tests/conformance.rs` (ignored without an installed Codex; `-j 1`): the installed version
  is the pinned one, its help documents every flag passed, it lists the `hooks` feature,
  Codex trusts every hook a launch subscribes, a delivered message's prompt reports through
  the bridge, a message joins a turn under way and a cancel interrupts it, a failed turn is
  followed to its outcome, and the launcher runs a whole conversation in tmux.
