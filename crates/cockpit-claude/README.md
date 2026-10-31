# cockpit-claude

The Claude Code provider adapter: its launch words, capabilities pinned to a tested version, terminal actions, screen-reading fallback and in-sandbox hook bridge.

Everything specific to Claude Code lives here; the daemon wires it in as a
`cockpit-provider-api` provider and nothing above the contract sees a Claude type (#33).

## Rules

- **Capabilities with provenance.** Resume, submit, events, outcome, cancel, effort,
  compact, remote control and screen reading are `tested` only when the running Claude Code
  is `CONFORMANCE_VERSION`; on any other or unknown version they are `discovered`
  (unverified). `events` and `outcome` need the hook bridge: an adapter wired without it
  subscribes no hooks. Commands, evidence and billing are not reported until their contract
  methods exist.
- **Launch.** A new conversation runs under a `--session-id` UUID the adapter mints, so it
  is named from the start; a resumed one runs `--resume <id>`. `--remote-control` comes
  first (its optional name would take the next plain word), the first message last after
  `--`. Every launch passes `--settings` with the hook bridge; the hooks therefore run only
  in this conversation, never in a nested `claude` a tool starts. Every launch also sets
  `CLAUDE_CODE_DISABLE_AGENT_VIEW=1`, which a nested `claude` inherits, so the conversation
  never moves into Claude Code's shared background daemon (`LAUNCH_ENVIRONMENT`, #184).
- **Hook bridge.** `cockpit-claude-hook <cockpit>` runs on `SessionStart`,
  `UserPromptSubmit`, `PostToolUse`, `Notification`, `Stop`, `StopFailure` and `SessionEnd`,
  normalizes the payload (`hooks::normalize`) and reports through `cockpit agent report`.
  It prints nothing on standard output and never exits 2. A question the agent channel
  would refuse is replaced by a plain one; line breaks become spaces and a long one is cut
  to 300 characters. A turn's end reports its outcome (`--outcome completed` or `failed`);
  a failure's reason the channel would refuse is left out, a long one cut to 80 characters.
  `StopFailure` `rate_limit` is `failed`, never `quota-stopped`: Claude Code sends it for a
  spent plan window and a transient 429 alike (#212). A `SessionStart` after a compaction
  reports nothing: an automatic one happens in the middle of a turn.
- **Terminal actions.** A message is typed then entered at the prompt, queued with Ctrl-X
  Enter during a turn, and refused while a dialog is open or the activity is unknown.
  Cancel presses Escape; compact types `/compact` at the prompt only.
- **Screen reading** is a fallback with recorded confidence (report 05): a permission dialog
  or a running spinner is `high`, a choice menu or the bare prompt `low`.

## Dependency edges

- Depends on: `cockpit-provider-api` only (crate-graph rule
  `provider adapters -> provider-api only`), `serde_json` for hook payloads.
- Used by: the daemon's wiring only (`capabilities -> provider adapters`).

## Tests

- `tests/launch.rs`, `tests/capabilities.rs`, `tests/input.rs`: the contract's words.
- `tests/hooks.rs`: conformance with the hook payloads pinned in
  `tests/fixtures/claude-code-2.1.316/`, and the reports they become.
- `tests/rate_limit.rs`: both kinds of `rate_limit` stop report a failed turn.
- `tests/screen.rs`: the screen-reading fallback.
- `tests/bridge.rs`: the hook bridge as a process against a fake `cockpit`.
- `tests/live_hooks.rs` (live test ignored without an installed Claude Code): a real launch's
  hooks report through the bridge with that launch's own token.
- `tests/discover.rs`: finding `claude` and its version.
- `tests/conformance.rs` (ignored without an installed Claude Code): the installed version is
  the pinned one and its `--help` documents every flag the adapter passes.
