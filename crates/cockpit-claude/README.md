# cockpit-claude

The Claude Code provider adapter: its launch words, capabilities pinned to a tested version, terminal actions, screen-reading fallback and in-sandbox hook bridge.

Everything specific to Claude Code lives here; the daemon wires it in as a
`cockpit-provider-api` provider and nothing above the contract sees a Claude type (#33).

## Rules

- **Sign-in.** `claude auth status` supplies JSON `loggedIn` plus exit 0/1. The probe has
  a ten-second deadline and a 64 KiB limit per output stream. Malformed or failed probes
  stay unknown; no raw output reaches readiness. `tests/sign_in_conformance.rs` proved
  signed-out and placeholder API-key states on 2.1.316 in an isolated Linux home.

- **Capabilities with provenance.** Resume, submit, events, outcome, cancel, effort, model,
  compact, remote control, screen reading, compaction threshold and quota are `tested` only when the running Claude Code
  is a version in `tested-versions.txt` (#732); on any other or unknown version they are
  `discovered` (unverified). `CONFORMANCE_VERSION` is only the version the recorded
  fixtures come from. `events`, `outcome` and `inbox` need the hook bridge: an adapter wired
  without it subscribes no hooks. Commands, evidence and billing are not reported until their contract
  methods exist.
- **Cost** (#699). `Provider::scan_replies` reads the assistant lines of
  `<home>/.claude/projects/**/*.jsonl` past each file's byte offset. Claude Code appends one
  line per content block with the whole reply's usage, so replies are keyed by message id and
  request id (a line without ids by its place in its file); the archive counts a key once and
  a resumed conversation's replay adds nothing. Only complete lines count: a line still being
  written is read by the next scan. A file shorter than its cursor was replaced and is read
  from its start. Links are not followed (the `.claude` folder, a subfolder or a transcript),
  at most 200 000 replies, 128 MiB of transcript and six folder levels are read per scan (a
  large history comes over several, each from its cursors), a transcript replaced by a longer
  one is not told from one that grew (Claude Code only appends), and a file that cannot be
  read is named in the scan's problems. A residual race remains between the length check and
  the open of a file replaced in between. The cache write split is read from
  `cache_creation` when present, else all of it counts as the five-minute rate; `speed:
  "fast"` is kept.
- **Quota** (#58). `Provider::quota` reads the plan windows of the account Claude Code is
  signed in to under the owner's home, from `https://api.anthropic.com/api/oauth/usage` with
  `anthropic-beta: oauth-2025-04-20`, the endpoint and header the owner's
  `config/waybar/.config/waybar/scripts/claude-usage.sh` uses. The token is read from
  `~/.claude/.credentials.json` at each request and handed to `curl` on its standard input
  (`--header @-`), never on its command line, in an error or to a session; an expired one is
  refused, never refreshed, since Claude Code owns it. Both answer shapes are read, the
  `limits` list and the older flat keys, into the same limit names: `five-hour`,
  `seven-day` and `seven-day:<model>` (a limit scoped to a model on a surface names both;
  a model listed twice is one window). A window with no share used yet is left out. A failed
  or limited request is refused in the adapter's words, without the answer or curl's own
  message. The host daemon alone calls it (#858); the hook bridge reads a session's own
  windows the same way at a `rate_limit` stop (#249, below).
- **Runtime probe** (#732). On a version not in `tested-versions.txt`, the daemon runs
  `probe` once at its start: each launch flag in `claude --help`, each launch environment name
  and hook event in the program. A missing requirement is logged and shown by `torlek doctor`
  with its name, and takes away the capabilities that need it; an extra flag changes nothing.
  A clean probe reports the capabilities `probed`, not `tested`. A launch asking for a
  capability the probe took away is refused with the missing requirement and the version
  (``flag --autocompact in `claude --help` is missing in 9.9.9``, #783).
- **Launch.** A new conversation runs under a `--session-id` UUID the adapter mints, so it
  is named from the start; a resumed one runs `--resume <id>`. `--remote-control` comes
  first (its optional name would take the next plain word), the first message last after
  `--`. `--effort` takes one of the levels the installed `claude --help` lists, read once per build
  by `Provider::check_choices` before a start and passed through when the help lists none (#743);
  `--model` takes Claude Code's own alias or
  model id, checked only for its shape, since Claude Code refuses a model the account cannot
  use (#701). A chosen compaction threshold is passed as `--autocompact <value>` in Claude
  Code's own words (`auto` or a context window such as `200k`), checked only for its shape;
  without one, nothing is passed and Claude Code's own setting applies (#783). Every launch passes `--settings` with the hook bridge; the hooks therefore run only
  in this conversation, never in a nested `claude` a tool starts. Every launch also sets
  `CLAUDE_CODE_DISABLE_AGENT_VIEW=1`, which a nested `claude` inherits, so the conversation
  never moves into Claude Code's shared background daemon (`LAUNCH_ENVIRONMENT`, #184), and
  `CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS=10000`, so a `SessionEnd` hook has the report's
  deadline instead of being killed after 1.5 s (#219).
- **Hook bridge.** `torlek-claude-hook <torlek>` runs on `SessionStart`,
  `UserPromptSubmit`, `PostToolUse`, `Notification`, `Stop`, `StopFailure`, `SessionEnd`,
  `PreCompact` and `PostCompact`, normalizes the payload (`hooks::normalize`) and reports through `torlek agent report`.
  Every report names the conversation it comes from (`--conversation`, the payload's
  `session_id`); a `SessionEnd` of `/clear` or an in-session `/resume` reports
  `--conversation-ends` for the conversation that ends, so the daemon follows the next one
  and refuses a report of any other (#219).
  A compaction reports `--compaction started|ended` with its trigger and, best effort, its
  sizes read from the transcript (`transcript::context_tokens`, `tokens_after`): the
  transcript is written after the hook runs, so a size that is not there yet is left out
  rather than guessed (#782). It proves no activity, and a `SessionStart` that follows one
  (`source: compact`) reports nothing and is not an unknown event.
  `Stop` and `StopFailure` add the harness's own context, `--context-tokens`, read the same
  way from the last main-chain model call (input plus cache read and creation); Claude Code
  names no window there, so none is sent (#479).
  It prints nothing on standard output and never exits 2. `torlek-claude-hook --version`
  answers its name and version: `Bridge::check`, run by the daemon at its start, proves the
  bridge runs here (#203). A question the agent channel
  would refuse is replaced by a plain one; line breaks become spaces and a long one is cut
  to 300 characters. A turn's end reports its outcome (`--outcome completed` or `failed`);
  a failure's reason the channel would refuse is left out, a long one cut to 80 characters.
  `StopFailure` `rate_limit` is ambiguous: Claude Code sends it for a spent plan window and
  a transient 429 alike (#212), so the bridge judges it by the plan windows of the session's
  own sign-in before reporting (#249): the usage request above, with the
  `CLAUDE_CODE_OAUTH_TOKEN` the session holds (the placeholder the session proxy swaps for
  the account's credential) or else the credentials file in its home, through the `curl` on
  its `PATH`. The `five-hour` or `seven-day` window used at or past its whole limit makes it
  `quota-stopped`, which moves the session to its next account (#59); a window scoped to one
  model is not read, since the payload does not say which model the turn ran on. Headroom in
  both, or windows that cannot be read, keep it `failed` with reason `rate_limit`, said on
  the hook's standard error. A `SessionStart` after a compaction
  reports nothing: an automatic one happens in the middle of a turn.
- **Inbox (#703).** On `SessionStart` and `Stop`, a second hook runs in the background
  (`asyncRewake`): `torlek-claude-hook <torlek> --inbox` waits on `torlek agent inbox` for a
  message from another session and wakes the model with it on standard error, exit status 2,
  never through the terminal. A newer waiter supersedes an older one, which ends quietly; a
  failure exits 1. Decided in `docs/decisions/2026-11-04-703-claude-message-inbox.md`.
- **Terminal actions.** The owner's message is typed then entered at the prompt, queued with Ctrl-X
  Enter during a turn, and refused while a dialog is open or the activity is unknown.
  Cancel presses Escape; compact types `/compact` at the prompt only. The keys reach the
  terminal in their own write after the text: Claude Code drops an Enter that arrives in the
  same burst as the text it reads as a paste (#730).
- **First-run screens (#1178).** Before each sandboxed launch, `prepare_session_home` writes
  `hasCompletedOnboarding: true` into the session home's `.claude.json` and, with an
  `ANTHROPIC_API_KEY` bound, approves the placeholder's last 20 characters in
  `customApiKeyResponses` (`src/first_run.rs`, measured on 2.1.319). A recorded answer, a
  "no" included, is kept.
- **Screen reading** is a fallback with recorded confidence (report 05): a permission dialog
  or a running spinner is `high`, a choice menu or the bare prompt `low`.
- **Sandbox needs** (`needs`, #830, #858). Hosts: `api.anthropic.com`,
  `mcp-proxy.anthropic.com`, `downloads.claude.ai`, `platform.claude.com`. Read from the
  owner's home: `~/.local/share/claude`, the installed versions. Written into the session's
  own folders: `~/.claude`, `~/.claude.json` and its backups, `~/.claude.lock`,
  `~/.cache/claude`, `~/.config/claude`, `~/.local/state/claude`, `~/.mcp.json`. Credential
  file `~/.claude/.credentials.json`; `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` take a
  bound credential, carried by `api.anthropic.com`. Seeded from Greywall's `claude.go` at
  `a9ccc58c` (`crates/cockpit-provider-api/NOTICE-greywall`). Left out of it: the macOS
  paths (macOS is not supported yet); `github.com`, `api.github.com` and
  `*.githubusercontent.com`, which the forge adapter declares for a project (#60, #857);
  `registry.npmjs.org`, a toolchain's registry (#859); `storage.googleapis.com`, a storage
  host every Google Cloud tenant shares, which Claude Code fetches its releases from while
  its install is read-only; and write access to `~/.local/share/claude`, since nothing writes the owner's
  home (#859).

## Dependency edges

- Depends on: `cockpit-provider-api` only (crate-graph rule
  `provider adapters -> provider-api only`), `serde_json` for hook payloads.
- Also uses the workspace-pinned `rustix` process API for sign-in probe and usage request
  cleanup, and the host's `curl` for the usage request: no HTTP crate.
- Used by: the daemon's wiring only (`capabilities -> provider adapters`).

## Tests

- `tests/needs.rs`: the declared hosts, folders and credentials, through the contract.
- `tests/launch.rs`, `tests/capabilities.rs`, `tests/input.rs`: the contract's words.
- `tests/hooks.rs`: conformance with the hook payloads pinned in
  `tests/fixtures/claude-code-2.1.316/`, and the reports they become.
- `tests/screen.rs`: the screen-reading fallback.
- `tests/replies.rs`: model replies from scratch homes holding transcripts shaped like Claude
  Code's (no test reads the owner's): tokens, cache lifetimes, fast mode, lines that cost
  nothing, cursors, partial lines, replaced files and links.
- `tests/quota.rs`: plan windows from both answer shapes in `tests/fixtures/oauth-usage/`,
  against a fake `curl` and a scratch home: the token's path, expiry and refusals.
- `tests/bridge.rs`: the hook bridge as a process against a fake `torlek`, and, for a
  `rate_limit` stop, a fake `curl` answering with the saved usage answers.
- `tests/bridge_check.rs`: `Bridge::check` passes the built bridge and refuses one that
  cannot run or is another program.
- `tests/conversation.rs`: the conversation each report names, and the end a `/clear` or an
  in-session `/resume` announces.
- `tests/live_hooks.rs` (live test ignored without an installed Claude Code): a real launch's
  hooks report through the bridge with that launch's own token.
- `tests/discover.rs`: finding `claude` and its version.
- `tests/conformance.rs` (ignored without an installed Claude Code): the installed version is
  the pinned one and its `--help` documents every flag the adapter passes.
