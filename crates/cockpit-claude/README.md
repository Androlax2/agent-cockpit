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
  compact, remote control and screen reading are `tested` only when the running Claude Code
  is `CONFORMANCE_VERSION`; on any other or unknown version they are `discovered`
  (unverified). `events`, `outcome` and `inbox` need the hook bridge: an adapter wired
  without it subscribes no hooks. Commands, evidence and billing are not reported until their contract
  methods exist.
- **Launch.** A new conversation runs under a `--session-id` UUID the adapter mints, so it
  is named from the start; a resumed one runs `--resume <id>`. `--remote-control` comes
  first (its optional name would take the next plain word), the first message last after
  `--`. `--effort` takes one of the levels the installed `claude --help` lists, read once by
  `Provider::check_choices` before a start and passed through when the help lists none (#743);
  `--model` takes Claude Code's own alias or
  model id, checked only for its shape, since Claude Code refuses a model the account cannot
  use (#701). Every launch passes `--settings` with the hook bridge; the hooks therefore run only
  in this conversation, never in a nested `claude` a tool starts. Every launch also sets
  `CLAUDE_CODE_DISABLE_AGENT_VIEW=1`, which a nested `claude` inherits, so the conversation
  never moves into Claude Code's shared background daemon (`LAUNCH_ENVIRONMENT`, #184), and
  `CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS=10000`, so a `SessionEnd` hook has the report's
  deadline instead of being killed after 1.5 s (#219).
- **Hook bridge.** `torlek-claude-hook <torlek>` runs on `SessionStart`,
  `UserPromptSubmit`, `PostToolUse`, `Notification`, `Stop`, `StopFailure` and `SessionEnd`,
  normalizes the payload (`hooks::normalize`) and reports through `torlek agent report`.
  Every report names the conversation it comes from (`--conversation`, the payload's
  `session_id`); a `SessionEnd` of `/clear` or an in-session `/resume` reports
  `--conversation-ends` for the conversation that ends, so the daemon follows the next one
  and refuses a report of any other (#219).
  It prints nothing on standard output and never exits 2. `torlek-claude-hook --version`
  answers its name and version: `Bridge::check`, run by the daemon at its start, proves the
  bridge runs here (#203). A question the agent channel
  would refuse is replaced by a plain one; line breaks become spaces and a long one is cut
  to 300 characters. A turn's end reports its outcome (`--outcome completed` or `failed`);
  a failure's reason the channel would refuse is left out, a long one cut to 80 characters.
  `StopFailure` `rate_limit` is `failed`, never `quota-stopped`: Claude Code sends it for a
  spent plan window and a transient 429 alike (#212). A `SessionStart` after a compaction
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
- Also uses the workspace-pinned `rustix` process API for sign-in probe cleanup.
- Used by: the daemon's wiring only (`capabilities -> provider adapters`).

## Tests

- `tests/needs.rs`: the declared hosts, folders and credentials, through the contract.
- `tests/launch.rs`, `tests/capabilities.rs`, `tests/input.rs`: the contract's words.
- `tests/hooks.rs`: conformance with the hook payloads pinned in
  `tests/fixtures/claude-code-2.1.316/`, and the reports they become.
- `tests/screen.rs`: the screen-reading fallback.
- `tests/bridge.rs`: the hook bridge as a process against a fake `torlek`.
- `tests/bridge_check.rs`: `Bridge::check` passes the built bridge and refuses one that
  cannot run or is another program.
- `tests/conversation.rs`: the conversation each report names, and the end a `/clear` or an
  in-session `/resume` announces.
- `tests/live_hooks.rs` (live test ignored without an installed Claude Code): a real launch's
  hooks report through the bridge with that launch's own token.
- `tests/discover.rs`: finding `claude` and its version.
- `tests/conformance.rs` (ignored without an installed Claude Code): the installed version is
  the pinned one and its `--help` documents every flag the adapter passes.
