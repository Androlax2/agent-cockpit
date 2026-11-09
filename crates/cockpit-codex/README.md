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
- **Quota** (#58, `quota`). `Provider::quota` reads the plan windows of Codex's quota
  sign-in, a sign-in of its own under the owner's `~/.local/share/codex-quota` where
  `config/waybar/.config/waybar/scripts/codex-quota.py` (`codex-usage.sh --login`) signs in,
  never the sessions' `~/.codex` (owner's decision, 2026-11-07). It runs `codex app-server -c
  cli_auth_credentials_store="file"` there on its standard streams, with `CODEX_HOME` and its
  folder set to that home, sends `initialize`, `account/read` (`refreshToken: false`) and
  `account/rateLimits/read`. Once both answer, within four fifths of 15 s, its input ends and
  it has a fifth of 15 s more to exit by itself, so a sign-in it rewrites is not cut short; then its
  process group is stopped. It gets `PATH`, `HOME` and `CODEX_HOME` only, none of the daemon's
  other variables. Codex alone reads its sign-in; no token passes through the adapter. A missing home or a
  sign-in that is not a subscription is refused with the command that signs in. The `codex`
  bucket (`rateLimitsByLimitId.codex`, or the older `rateLimits`) gives up to two windows keyed
  by length: `five-hour`, `seven-day`, `window-<minutes>-minutes`, or the slot's name without a
  length. `ordinaryUsageAllowed: false` is a blocked reading. Codex's error text is not
  repeated. The fields are those of `codex app-server generate-ts` on 0.166.0. The host daemon
  alone calls it (#858). `XDG_DATA_HOME` is not followed: the home is under `~/.local/share`.

- **Capabilities with provenance.** Resume, model, effort, compaction threshold, quota and, with the bridge, events, submit, cancel and
  outcome are `tested` only when the running Codex is a version in `tested-versions.txt`
  (#732); on any other or unknown version they are `discovered` (unverified).
  `CONFORMANCE_VERSION` (0.166.0) is only the version the recorded fixtures come from. Compact, billing, commands,
  evidence, remote control and screen reading are untested and not reported.
- **Runtime probe** (#732). On a version not in `tested-versions.txt`, the daemon runs
  `probe` once at its start: the launch flags, `--config` among them, in `codex --help` and `codex resume --help`, the
  server flags in `codex app-server --help`, the hooks feature in `codex features list`, and
  the compaction threshold key: `codex --config model_post_turn_compact_threshold_percent=torlek-probe
  features list` names the key in its refusal when Codex knows it, and says nothing when it
  does not (#783). A missing requirement is named and takes away the capabilities that need it
  (the hooks feature takes away what the bridge adds to resume; the key, the compaction
  threshold only); a launch asking for a capability taken away is refused with that
  requirement and the version.
- **Launch with the bridge** (#407). The session runs `torlek-codex-session`, which starts
  the session's private app-server, `codex app-server --listen unix://<session dir>/codex.sock
  --enable hooks --config <hook>…` (`server_arguments`), then the TUI against it, `codex
  [resume] --remote unix://<socket> [--config <value>]… [-- [<thread id>] [<first message>]]`
  (`tui_arguments`).
  Both run in the session's terminal with the launch's environment, so hooks keep the launch
  token; the server's working directory is the conversation's. When the TUI exits, the
  launcher stops the server (`SIGTERM`, which Codex's npm wrapper passes on, then `SIGKILL`
  to the wrapper and its descendants) and exits with the TUI's status. The server's output goes to `codex-app-server.log` beside
  the socket. A private server per session, never Codex's shared one: the shared server
  runs hooks with whichever launch's token started it (as #184 for Claude Code).
- **Launch without the bridge.** `codex [resume] --no-daemon`, with the thread id and the
  first message after `--`, where neither can be read as a flag; only resume, the
  compaction threshold and quota are reported.
- **Model and effort** (#1281). A chosen model and effort are passed as `--config
  model="<model>"` and `--config model_reasoning_effort="<effort>"` (`session_configs`), to both
  the app-server and the TUI with the bridge (the TUI through the launcher's `--tui-config`), so
  neither falls back to the owner's `~/.codex/config.toml`; to the TUI without the bridge. On
  0.166.0, `thread/start` answers with them over the home's own `model`. Only their shapes are
  checked: Codex takes any model id and accepts any effort word at `thread/start`, refusing one
  its model lacks only when a turn runs. Without them, nothing is passed.
- **Sandbox** (#1281, owner's decision 2026-11-09). Every launch overrides the owner's
  `sandbox_mode`: `--config sandbox_mode="workspace-write"` and `--config
  sandbox_workspace_write.network_access=true` (`SANDBOX_CONFIGS`), with the model and effort in
  `session_configs`, so the app-server and the TUI. Measured on 0.166.0: without them an
  untrusted clone runs read-only; without the network Codex's sandbox refuses every `connect`
  (`EPERM`), Torlek's agent socket included, and adding the socket's folder to `writable_roots`
  does not change that, nor does the experimental network proxy's `allow_unix_sockets`. Torlek's
  own sandbox, proxy and grants stay the boundary. The owner's approval policy and
  `writable_roots` are not set: a `--config` array replaces the owner's list instead of adding
  to it. Codex sessions started by hand are untouched.
- **Update screen** (#1281). Every launch gives the TUI `--config
  check_for_update_on_startup=false` (`UPDATE_CHECK_CONFIG`): Codex's "Update available" screen
  held a restarted session until the owner answered it in the terminal. On 0.166.0 the screen
  appears only for a launch without a first message (a restart or a resume); a first message
  skips it by itself. The app-server shows no screen and does not get it.
- **Compaction threshold** (#783). A chosen threshold is passed as `--config
  model_post_turn_compact_threshold_percent=<percent>`, Codex's own setting in its own words
  (a whole percent of the usable context window, `0` off), to the app-server with the bridge
  and to the TUI without; the adapter refuses anything but 0 to 100 before the launch. Without
  one, nothing is passed and Codex's own configuration applies.
- **Thread ids.** Codex mints a new conversation's thread id itself, so a new launch names no
  conversation: the first report does (`--conversation`).
- **Hooks with launch-scoped trust.** Codex runs a hook only when its trusted hash is recorded.
  The app-server gets `--enable hooks` and, with `--config`, each subscribed hook plus a
  `hooks.state` entry trusting exactly that hook's hash (`hooks::subscription`). Codex files
  them under its session-flags source, so they apply to this session's server only, never to
  a nested `codex` a tool starts, and the owner's persisted trust and the project's untrusted
  hooks stay as they are; `--dangerously-bypass-hook-trust` is never passed.
- **Hook bridge.** `torlek-codex-hook <torlek>` runs on `SessionStart`, `UserPromptSubmit`,
  `PostToolUse`, `PermissionRequest`, `Stop`, `Interrupt`, `PreCompact` and `PostCompact`,
  normalizes the payload
  (`hooks::normalize`) and reports through `torlek agent report`, naming the thread it comes
  from (`--conversation`, the payload's `session_id`). A permission request reports a plain
  `--dialog` question, never the tool input; `Stop` reports `waiting --outcome completed`,
  `Interrupt` reports `waiting`. A compaction reports `--compaction started|ended` with its
  trigger and, best effort, the sizes read from the thread's rollout (`rollout`): the first
  `token_count` after the latest `compacted` record is the size after it (#782). `Stop` adds
  the harness's own context from the rollout's last `token_count`:
  `--context-tokens` (`last_token_usage.input_tokens`) and `--context-window`
  (`model_context_window`) (#479). It prints nothing on standard output, so it never answers a
  permission request, and never exits 2. A subagent's (`agent_id`) activity and a
  `SessionStart` after a compaction report nothing; a subagent's permission request still
  reports `--dialog`. `torlek-codex-hook --version` and `torlek-codex-session --version`
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
  `/tmp/codex-daemon-<uid>/` whatever `TMPDIR` says, beside the owner's own Codex's. An
  unsandboxed launch reaches it directly; a sandbox backend must show a folder of the
  session's own there, and when the reach recorded at launch says it cannot, the daemon
  refuses delivery naming why (#446, #828). Where the approval
  requests of a delivered turn go is measured with a local fake model on 0.166.0 below;
  signed-in delivered-start acceptance passed on Linux with 0.166.0
  ([#436 receipt](../../docs/acceptance/436-codex-approval-routing.md)). Older versions are unverified.
- **Sandbox needs** (`needs`, #830, #858). Hosts: `api.openai.com`, `chatgpt.com`,
  `ab.chatgpt.com`, and `auth.openai.com`, which refreshes a ChatGPT sign-in's tokens and was
  not in Greywall's profile. Nothing read from the owner's home; written into the session's own
  folders: `~/.codex`, `~/.cache/codex`. Credential file `~/.codex/auth.json`;
  `OPENAI_API_KEY` or `CODEX_API_KEY`, which Codex 0.166.0 names as its supported key
  variables, take a bound credential, carried by `api.openai.com`. Whether its TUI takes a
  placeholder there is #832's conformance test; if not, #832 writes a session-own
  `auth.json` holding only the placeholder. Seeded from Greywall's `codex.go` at `a9ccc58c`
  (`crates/cockpit-provider-api/NOTICE-greywall`); its macOS paths are left out (macOS is not
  supported yet) and its read of `~/.codex` and `~/.cache/codex` becomes the session's own
  folders.

## Dependency edges

- Depends on: `cockpit-provider-api` only (crate-graph rule
  `provider adapters -> provider-api only`), `serde_json` for hook payloads and JSON-RPC,
  `sha2` for the hook trust hash, `rustix` for the launcher's `SIGTERM` and the quota
  reader's process group.
- Used by: the daemon's wiring only (`capabilities -> provider adapters`), which comes with
  #411 together with shipping `torlek-codex-hook` and `torlek-codex-session` in the release.

## Tests

- `tests/needs.rs`: the declared hosts, folders and credentials, through the contract.
- `tests/launch.rs`, `tests/capabilities.rs`: the contract's words, and the app-server's and
  TUI's.
- `tests/hooks.rs`: conformance with the payloads and hook input schemas pinned in
  `tests/fixtures/codex-0.161.0/`, and the reports they become. Captures retain their
  original provenance; [0.166.0 evidence](tests/fixtures/codex-0.166.0/README.md) records
  the identical upstream schema blobs and separate native observations.
- `tests/trust.rs`: the subscription's `--config` values and the trust hash.
- `tests/bridge.rs`: the hook bridge as a process against a fake `torlek`.
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
- `tests/quota.rs`: plan windows against a fake app-server in a scratch home: both answer
  shapes, keys by length, the quota home and its words, the requests, the server stopped,
  and each refusal; ignored, against the installed harness in an empty isolated quota home
  (`COCKPIT_CODEX_PROGRAM`): both reads answer and the home is not signed in.
- `tests/conformance.rs` (native checks ignored by default; `-j 1`): the installed version
  is a Codex (the nightly job runs the latest, #732, and checks the version it installed), its help documents every flag passed, it lists the `hooks` feature,
  Codex trusts every hook a launch subscribes, a delivered message's prompt reports through
  the bridge, a message joins a turn under way and a cancel interrupts it, a failed turn is
  followed to its outcome, a thread runs the model, effort and sandbox a launch chooses, a
  command in that sandbox reaches a Unix socket outside its workspace, and the
  launcher completes and resumes the same conversation and skips the update screen in private
  tmux. Children have cleared environments and scratch homes; successful and
  gated-error responses come from a loopback fixture without credentials or account use.
  Ordinary tests check environment isolation and the local response scenarios.
  Native terminal checks require Linux `/proc` for verified pane-group cleanup;
  `a_failed_terminal_check_stops_descendants_that_ignore_hangup` exercises the
  failure path with fake processes in private tmux, without running Codex.

## Delivered-turn approval conformance (#436)

`tests/approval_routing.rs` exercises `Provider::deliver`, a recording Unix-socket proxy,
the installed app-server and its real TUI in a private tmux server. Its ordinary tests
check framing, connection identity, disconnect observation and helper-process bounds.
The ignored isolated tests of `tests/conformance.rs` run on any Codex version; the others pin **0.166.0**, the installed version observed on 2026-11-02.
The separate capability suite now validates that same production pin; approval routing
alone is not evidence for the other capabilities.

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

Production delivery stays unchanged: on 0.166.0 the evidence does not support a persistent
approval owner or refusing delivery, because the TUI answers the approval itself after the
delivery connection closes. Cockpit sends no approval response, and the proxy forwards
bytes unchanged, so only the real TUI can supply a cancellation. A fixture built from a
protocol schema alone would not prove this routing.

Why the probe is built this way: empty TOML tables merge with the owner's config, so
`mcp_servers={}` cannot disable servers, and the live probe asks Codex for its effective
config instead; `config/read` without `cwd` omits project layers, so both modes pass the work
directory; Escape can be rebound to approval, so custom keymaps are refused; `notify=[]`
disables legacy notify commands for both processes. Codex 0.166.0 drains pending input at a
protected screen boundary for up to one second, so the probe waits for the prompt and lets
that interval pass before Escape. The TUI's title helper is a tool-less conversation and must
not be counted as the delivered turn.

The signed-in test is **separate from the account-free proof**. It passed on Linux
on 2026-11-02; see the [acceptance receipt](../../docs/acceptance/436-codex-approval-routing.md).
Run it only after explicit host approval
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
configuration layers, managed requirements, concrete TUI bindings/unbindings, custom providers/endpoints,
and a missing signed-in OpenAI account. `config/read` receives the actual work directory;
the same setup guards apply to isolated tests, which also verify their local model URL.
The live probe checks `account/read` without requesting a token refresh. Refusal requires a separate live-run
plan, not edits to the owner's policy or credentials. The probe sets read-only sandboxing
and human approval, only cancels through the TUI, and never accepts a command. Version and
tmux commands have deadlines and bounded output, including teardown. Live transcripts
and server diagnostics are withheld from test output. The environment marker is an opt-in,
not a substitute for host authorization. Each additional run needs fresh approval;
the recorded success covers its exact version, platform and configuration.

On 2026-11-02, the host approved one run of the compiled signed-in test on 0.166.0.
It exited **101** with `custom TUI keymaps need a separate conformance plan; no turn
started`. The configured home was not changed to bypass that guard. No model turn ran;
that receipt remains a failed attempt.

Subsequent account-free resume checks showed that 0.166.0 serializes default keymap
slots as null after a TUI session. The guard now accepts only object/null defaults;
concrete bindings and arrays (including empty unbindings) remain refused. This repair
does not reclassify the earlier refused signed-in attempt or authorize a retry.

After PR #576 merged, a separately approved run refused inherited execution rules
before a turn. Theo then selected a private validation home, signed in through the
official CLI, and approved one new run: `edeea0394e55` passed (1 passed, 19 skipped,
exit 0). The delivered start reached the persistent TUI, which returned `cancel`;
the matching turn ended interrupted. Both earlier refusals remain historical.
