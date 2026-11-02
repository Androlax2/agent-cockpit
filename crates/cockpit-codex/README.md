# cockpit-codex

The Codex provider adapter: its launch words, capabilities pinned to a tested version, thread ids, the in-sandbox hook bridge with launch-scoped hook trust, and delivery through a private app-server per session.

Everything specific to Codex lives here; the daemon wires it in as a `cockpit-provider-api`
provider and nothing above the contract sees a Codex type (#48).

## Rules

- **Sign-in.** `codex login status` supplies local credential status. Exit 1 alone is not
  proof of being signed out: it must say `Not logged in`. Both the older `Logged in using`
  form and the newer `API key configured` form are recognized; raw output, including any
  masked key, is discarded. The probe has a ten-second deadline and a 64 KiB output limit
  per stream. `tests/sign_in_conformance.rs` proved signed-out and placeholder API-key states
  on `SIGN_IN_CONFORMANCE_VERSION` (0.166.0), independently of the delivery capability pin.

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
  The socket is opened only through the caller's `SessionSockets`, never by its path: the
  agent writes the session's directory, so `codex.sock` names whatever the agent linked there
  (#442). The daemon's check is `cockpit-daemon`'s `SessionTreeSockets`.
- **Conversation changes** (#408). The launcher creates a private, fresh report ledger for
  its hooks and failed-turn watcher. They serialize reports under its file lock. A root
  `SessionStart` with `source: clear` announces the recorded thread's `--conversation-ends`
  before reporting the new thread. A successful end retires the old thread even if the
  successor report fails, so late reports cannot consume the pending handoff. A retry or
  later prompt can finish it. The daemon still authenticates
  every report and checks its conversation. In-session resume/fork behavior is unmeasured.
- **Known gaps.** The socket's real file is Codex's, under
  `/tmp/codex-daemon-<uid>/` whatever `TMPDIR` says, beside the owner's own Codex's: the
  sandbox shows a folder of the session's own there, through a greywall fork feature, and
  without it the daemon refuses delivery naming that feature (#446). Where the approval
  requests of a delivered turn go is measured with a local fake model on 0.166.0 below;
  signed-in acceptance and the older 0.161.0 target remain unmeasured (#436).

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
- `tests/clear.rs`: repeated clears, duplicate and late hooks, failed handoff retries and
  launch isolation against a channel double enforcing conversation changes.
- `tests/bridge_check.rs`: `Bridge::check` passes the built programs and refuses one that
  cannot run or is another program.
- `tests/delivery.rs`: submit, steer and cancel against a fake app-server
  (`tests/common/app_server.rs`).
- `tests/turns.rs`: a failed turn's outcome and report.
- `tests/session.rs`: the session launcher as a process with a fake Codex: the server and TUI
  words, failed-turn reports, the server stopped with the TUI; a stopped test launcher
  leaves no fake process behind (#440).
- `tests/discover.rs`: finding `codex` and its version.
- `tests/conformance.rs` (ignored without an installed Codex; `-j 1`): the installed version
  is the pinned one, its help documents every flag passed, it lists the `hooks` feature,
  Codex trusts every hook a launch subscribes, a delivered message's prompt reports through
  the bridge, a message joins a turn under way and a cancel interrupts it, a failed turn is
  followed to its outcome, and the launcher runs a whole conversation in tmux.

## Delivered-turn approval conformance (#436)

`tests/approval_routing.rs` exercises `Provider::deliver`, a recording Unix-socket proxy,
the installed app-server and its real TUI in a private tmux server. Its ordinary tests
check framing, connection identity, disconnect observation and helper-process bounds.
The ignored isolated tests pin **0.166.0**, the installed version observed on 2026-11-02;
they do not change `CONFORMANCE_VERSION` (0.161.0) or mark other capabilities tested.

```sh
CARGO_BUILD_JOBS=2 COCKPIT_CODEX_PROGRAM=/path/to/codex \
  cargo nextest run --locked -p cockpit-codex --test approval_routing \
  --run-ignored only -E 'not test(a_signed_in_)' --no-capture
```

`--no-capture` runs these serially. Both isolated scenarios passed on Linux: a delivered
start and a delivered steer reach the TUI with `item/commandExecution/requestApproval`
after their delivery connection closes. The TUI returns `cancel`, the server resolves
that request, and the matching turn ends `interrupted`. Cockpit sends no approval response.
The fake model only requests `/usr/bin/printf cockpit-approval-fixture`; it uses loopback
HTTP, fresh homes, no auth files, and no account usage. The TUI's tool-less title helper is
kept separate from the root conversation. The recorder does not modify traffic.
Credential-free native regressions also check refusal of enabled MCP servers, inherited
execution rules, custom providers, signed-out homes, project settings and custom keymaps.
A configured notification sentinel
does not run during the isolated turn. The probe records a disconnect only after shutting
down its upstream socket and matches the connection carrying the specific start or steer.

The signed-in test is **not part of this proof**. Run it only after explicit host approval
(agents use `ask-to-run`), with the exact installed version and an existing signed-in home:

```sh
COCKPIT_CODEX_PROGRAM=/path/to/codex \
COCKPIT_CODEX_SIGNED_IN_HOME=/absolute/signed-in/codex-home \
COCKPIT_CODEX_ALLOW_SIGNED_IN=1 CARGO_BUILD_JOBS=2 \
  cargo nextest run --locked -p cockpit-codex --test approval_routing \
  --run-ignored only -E 'test(=a_signed_in_delivered_turn_reaches_the_real_tui)' --no-capture
```

This uses the home's configured model and account, sends an initial READY prompt and one
delivered command request, and may generate a title. It consumes account usage and writes
Codex conversation/state records in that home. It never copies credentials into fixtures.
The work directory, other home directories, sockets and tmux server are temporary. Hooks,
legacy notification commands, plugins, apps, memories and shell snapshots are disabled for
these test processes. Before a thread starts, the probe refuses enabled MCP servers,
inherited execution rules, nonempty project layers (including disabled ones), unsupported
configuration layers, managed requirements, custom TUI keymaps, custom providers/endpoints,
and a missing signed-in OpenAI account. `config/read` receives the actual work directory;
the same setup guards apply to isolated tests, which also verify their local model URL.
The live probe checks `account/read` without requesting a token refresh. Refusal requires a separate live-run
plan, not edits to the owner's policy or credentials. The probe sets read-only sandboxing
and human approval, only cancels through the TUI, and never accepts a command. Version and
tmux commands have deadlines and bounded output, including teardown. Live transcripts
and server diagnostics are withheld from test output. The environment marker is an opt-in,
not a substitute for host authorization. No signed-in result is claimed until this command
has actually run with approval; keep #436 open meanwhile.
