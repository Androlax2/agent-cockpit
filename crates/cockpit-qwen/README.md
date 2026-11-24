# cockpit-qwen

The Qwen Code provider adapter: its launch words, capabilities, terminal actions and the in-sandbox hook bridge.

Everything specific to Qwen Code lives here; the daemon wires it in as a `cockpit-provider-api`
provider and nothing above the contract sees a Qwen Code type (#48). The adapter was mapped
from Qwen Code's own source and documentation at **0.28.4** (tag `v0.28.4`, commit
`e8424c88ddd463c56602063efde57c328b917cd4`); the mapping is on #1446, which added it.

## Supported version

Qwen Code **0.28.4**. `tested-versions.txt` lists no version yet (owner's decision on #1446):
every capability is `discovered` until the live conformance test passes on 0.28.4 and the
version is added. On another version the daemon runs the probe (each launch flag in
`qwen --help`); a clean probe makes the capabilities `probed`.

## Rules

- **Launch.** `qwen --session-id <uuid>` for a new conversation, with a UUID the adapter mints
  so the conversation is known from the start, or `qwen --resume <id>`; then `--model <id>` when
  one is chosen, and the first message as one word, `--prompt-interactive=<message>`, so a
  message starting with `-` is never a flag. A model is checked for its shape only: Qwen Code
  refuses one that no auth type or `modelProviders` entry offers.
- **Hooks without touching a settings file.** Qwen Code has no `--settings` flag. A launch with
  the bridge sets `QWEN_CODE_SYSTEM_DEFAULTS_PATH` to the bridge's defaults file, the lowest of
  Qwen Code's settings scopes, which is not gated by folder trust. The daemon writes the file at
  its start (`defaults_settings`), in its state directory, which agents read only; it runs
  `torlek-qwen-hook <torlek>` on every event of `hooks::HOOK_EVENTS` and turns the update check
  off (the install is read-only in the sandbox). Qwen Code merges hooks of every scope, so the
  owner's and the project's own hooks run beside the bridge, and every other setting of theirs
  wins over these defaults. A `qwen` the agent starts itself inherits the variable; its reports
  name another conversation and the daemon refuses them (#219).
- **Bridge** (`torlek-qwen-hook`). It normalizes each payload like `cockpit-claude`'s hooks:
  `SessionStart` started (or compacted when its source is `compact`), `UserPromptSubmit` and
  `PostToolUse`/`PostToolUseFailure` working, `Notification` `permission_prompt` needs you and
  `idle_prompt` idle, `Stop` completed, `StopFailure` failed with Qwen Code's `error` as the
  reason, `PreCompact`/`PostCompact` a compaction. `rate_limit` stays `failed`: Qwen Code gives it
  for any 429, so it never moves a session to another account. `Stop`'s `input_tokens` and
  `context_limit` are the turn's `--context-tokens` and `--context-window`. A `SessionEnd` of
  `/clear` announces `--conversation-ends`. On every `SessionStart` it prints `torlek brief` as
  the hook's `additionalContext` (#419). `Bridge::check` at the daemon's start proves the
  defaults file holds this bridge's hooks and the hook answers this release's `--version`.
- **Actions.** Submit types the message then Enter while Qwen Code waits, and the message then
  Ctrl-Q while it works (Enter there would steer the running turn); it is refused in a dialog
  or when the activity is unknown. Cancel is Escape. Compact types `/compress` while it waits.
- **Sign-in** is always `unknown`, next step "start qwen and run /auth, or set the provider key"
  (owner's decision on #1446): 0.28.4 removed `qwen auth`, and a sign-in may be the OAuth file,
  a key variable, a `.env` file, the owner's `settings.json`, or nothing for a local server.
- **Endpoint** (owner's decision on #1446). The adapter passes `--model` only; the endpoint
  comes from Qwen Code's own settings, such as the project's `.qwen/settings.json`
  `modelProviders` `baseUrl`. A local OpenAI-compatible server is reached through a project
  `[[services]]` forward the owner grants, listening on `127.0.0.1:<port>` in the sandbox, whose
  `NO_PROXY` already covers loopback.
- **Project instructions.** Qwen Code reads `QWEN.md` and `AGENTS.md` upward from the working
  directory by default, so a project's `AGENTS.md` needs nothing. `~/.qwen/QWEN.md` is the
  default start file (`cockpit-config`).
- **Needs** (`needs`). Hosts: Qwen OAuth's (`api.qwen.ai`, `auth.qwen.ai`, `chat.qwen.ai`) and
  each region of the first-party key presets (DashScope, Coding Plan, Token Plan, QwenCloud and
  its Token Plan). Writes `~/.qwen` and `~/.cache/qwen-code` in the session's own home; reads
  the standalone installer's `~/.local/lib/qwen-code`. Credentials: `~/.qwen/oauth_creds.json`,
  and each preset's key variable (`DASHSCOPE_API_KEY`, `BAILIAN_CODING_PLAN_API_KEY`,
  `BAILIAN_TOKEN_PLAN_API_KEY`, `QWENCLOUD_API_KEY`, `QWENCLOUD_TOKEN_PLAN_API_KEY`) as Bearer on
  its own hosts. Left out: the usage statistics host, `models.dev` and `registry.npmjs.org`. An
  npm install under the home, or another provider's host, is the owner's entry to add.

## What does not work yet

- **Inbox** (#703): Qwen Code has no hook that wakes an idle model, so messages from other
  sessions are not collected; #1447 tracks it, through `--input-file`.
- An in-session `/resume` or `/branch` sends no `SessionEnd`, so the reports of the
  conversation it switches to are refused until the session is relaunched. A `/clear`'s
  `SessionEnd` is not awaited by Qwen Code, so its report can arrive after the next
  `SessionStart`'s.
- Effort, compaction threshold, remote control, quota, cost, native commands and screen reading
  are not reported; folder trust is not kept (`trust_workspace` is `NotKept`; Qwen Code's
  folder trust is off by default).
- The nightly harness check (`tools/harness_conformance.py`) does not install Qwen Code yet.

## Tests

`tests/` drive the adapter through its public API with fake `qwen` programs and the payloads in
`tests/fixtures/qwen-code-0.28.4/`; nothing runs an installed Qwen Code or reads a sign-in.
`tests/conformance.rs` is ignored: it reads an installed Qwen Code's `--version`, `--help` and
package files, and needs no account. The owner runs it on 0.28.4 before the version is listed:

```sh
COCKPIT_QWEN_PROGRAM="$(command -v qwen)" \
  cargo nextest run --locked -p cockpit-qwen --test conformance --run-ignored only
```
