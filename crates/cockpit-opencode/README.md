# cockpit-opencode

The OpenCode provider adapter: its launch words with a standalone server per session, capabilities pinned to a tested version, terminal actions, the server's native commands, inbox delivery and variants, and the in-sandbox plugin bridge.

Everything specific to OpenCode lives here; the daemon wires it in as a `cockpit-provider-api`
provider and nothing above the contract sees an OpenCode type (#47).

## Rules

- **Sign-in.** `opencode auth list` reports saved credentials and environment credential
  counts, using the output documented by 1.19.5's
  [providers command](https://github.com/anomalyco/opencode/blob/v1.19.5/packages/opencode/src/cli/cmd/providers.ts).
  A positive count is signed in; zero names `opencode auth login` or a credential-free
  model as the next step. This does not prove that a selected model works. Failed or
  unfamiliar output is unknown. A ten-second deadline and 64 KiB per output stream bound
  the probe; raw output never leaves the adapter. Provenance remains `discovered`:
  `tests/sign_in_conformance.rs` needs the pinned harness, which was unavailable for #443.

- **Runtime probe** (#732). On a version not in `tested-versions.txt`, the daemon runs
  `probe` once at its start: each launch flag in `opencode --help` and each environment
  variable the adapter sets in the program. A missing one is named and takes away every
  capability, since every launch uses them (OpenCode 2.2.5 no longer has `--port`,
  `--hostname` or `OPENCODE_SERVER_USERNAME`). The `compaction.buffer` setting's name in the
  program is probed too; its loss takes away the compaction threshold alone, and a launch
  asking for it is then refused with that requirement and the version (#783).
- **Capabilities with provenance.** Every capability is `tested` only when the running
  OpenCode is a version in `tested-versions.txt` (#732); on any other or unknown version it
  is `discovered` (unverified). `CONFORMANCE_VERSION` is only the version the recorded
  fixtures come from. Without the plugin bridge only resume and the compaction threshold are offered: no events means the
  activity is never known, so submit and cancel would always be refused. Effort is not
  reported: the TUI sends the variant it saved for the model with every prompt, so a
  launch cannot choose one; a variant goes with a message the server delivers instead. Compact and screen reading are not reported. Commands are
  reported with the bridge and served through `Provider::commands` and `Provider::invoke_command`.
- **Standalone server per session.** Every launch runs the TUI with its own server on a free
  loopback port (`--port`, `--hostname 127.0.0.1`), behind a password minted for it
  (`OPENCODE_SERVER_PASSWORD`, user `opencode`). No session shares a server, and no other
  local program reaches one without its password. `Server::from_launch` reads both back
  from the launch's words. A port taken between the adapter's check and OpenCode's start
  fails the launch rather than sharing a server.
- **Launch.** A resumed conversation runs `--session <id>` (an id of the form `ses…`); a new
  one runs unnamed, because OpenCode creates its session at the first prompt and refuses a
  `--session` it does not know: the bridge's first report names it. The first message goes
  as one word, `--prompt=<message>`, so a message starting with `-` is not a flag.
- **Compaction threshold** (#783). A chosen threshold is OpenCode's own `compaction.buffer`
  in the launch's `OPENCODE_CONFIG_CONTENT`, over the owner's `compaction` table: a percent of
  the context window kept free (`15%`, as a string) or a token count (`20000`, as a number);
  anything else is refused before the launch. It is set with or without the bridge; without
  one, nothing is set and OpenCode's own configuration applies.
- **Plugin bridge.** With a bridge, a launch sets `OPENCODE_CONFIG_CONTENT` to load
  `plugin/cockpit-opencode-plugin.js` (`PLUGIN_SOURCE`, written by the daemon where
  `Bridge::plugin` says) on top of the owner's configuration (an `OPENCODE_CONFIG_CONTENT`
  the owner set is kept, the plugin joining its list; one it cannot join refuses the launch), and tells it the bridge, the
  `torlek` program and its server's port. The plugin forwards `session.created`,
  `session.status` (`busy`, `idle`), `permission.asked`, `question.asked`, a tool part that
  completed or failed, an assistant message that ended its turn and
  `session.compaction.started`, `ended` and `failed`, in order, to
  `torlek-opencode-bridge <torlek>`, which normalizes the event (`events::normalize`) and
  reports through `torlek agent report --conversation <session>`. Subagents' child sessions
  are dropped, except the permission and question dialogs they open, which wait in the same
  terminal and are reported under the root session. The plugin stays silent in an `opencode` whose server is not its launch's.
  A turn ends `completed` when the assistant message finishes for a reason OpenCode's loop
  stops on, `failed` with its error's name otherwise; an `APIError` is never
  `quota-stopped`, and a turn the owner interrupted (`MessageAbortedError`) has no outcome.
  The bridge gives up on a report after 5 seconds, so a stalled one never holds back the
  next event. Questions and reasons are cut to what the agent channel accepts.
  A compaction reports `--compaction` with the `reason` OpenCode names as its trigger and the sizes its
  `ended` event carries (tokens before: input plus cache; after: output). OpenCode 2.2.5
  puts these events' payload under `data`, not `properties`; both are read (#782).
- **Terminal actions.** A message is typed then entered while it waits or works (OpenCode
  queues it behind the turn), refused while a dialog is open or the activity is unknown,
  and refused when typing it would open a menu (`/`, `!` first, any `@`) or quit
  (`exit`, `quit`, `:q`); `Server::deliver` sends those as written. Cancel presses Escape
  twice during a turn only: the TUI aborts on the second.
- **Server API.** `Server::commands` lists the native commands (`GET /command`),
  `Server::invoke_command` runs one in a session and returns the id of the message it
  answered with, `Server::deliver` delivers an inbox message (`prompt_async`, accepted at
  once), `Server::variants` lists a model's variants, `Server::abort` ends a turn. Session
  ids and variant names are checked before they reach a URL or a body.
- **Provider delivery** (#416). `Provider::deliver` uses the host-retained launch command in
  `DeliveryTarget::launch` to recover the server. Submit sends literal text through
  `prompt_async` and returns `Delivered::Accepted`; a queued message proves no turn started.
  Cancel uses `abort`, requiring its affirmative response. Native commands keep their source
  labels and run through the same server, with a 10-second response deadline. Missing or
  malformed launch metadata and server refusals stay errors; no terminal fallback occurs
  inside this adapter. The daemon's delivery calls and launch retention are owned by #431.
- **Sandbox needs** (`needs`, #830, #858). Hosts: `api.openai.com`,
  `api.anthropic.com`, and `models.dev`, the model catalog OpenCode fetches at start, which
  was not in Greywall's profile. Read from the owner's home: `~/.opencode`, its program and packages.
  Written into the session's own folders: `~/.opencode.json`, `~/.config/opencode`,
  `~/.cache/opencode`, `~/.local/share/opencode`, `~/.local/share/opentui`,
  `~/.local/state/opencode`. Credential files: `~/.local/share/opencode/auth.json` and
  `account.json`, and the GitHub Copilot sign-in `~/.config/github-copilot`, which Greywall
  let it read and which is no longer bound. `ANTHROPIC_API_KEY` is carried by
  `api.anthropic.com`, `OPENAI_API_KEY` by `api.openai.com`. Seeded from Greywall's
  `opencode.go` at `a9ccc58c` (`crates/cockpit-provider-api/NOTICE-greywall`); write access
  to `~/.opencode` is left out, since nothing writes the owner's home (#859).

## Dependency edges

- Depends on: `cockpit-provider-api` only (crate-graph rule
  `provider adapters -> provider-api only`), `serde_json` for events and the server's JSON.
- Also uses the workspace-pinned `rustix` process API for sign-in probe cleanup.
- Used by: the daemon's wiring only (`capabilities -> provider adapters`).

## Tests

- `tests/needs.rs`: the declared hosts, folders and credentials, through the contract.
- `tests/launch.rs`, `tests/capabilities.rs`, `tests/input.rs`: the contract's words.
- `tests/events.rs`: conformance with the events pinned in `tests/fixtures/opencode-1.19.5/`,
  and the reports they become.
- `tests/bridge.rs`: the bridge as a process against a fake `torlek`.
- `tests/bridge_check.rs`: `Bridge::check` passes the built bridge and this release's
  plugin, and refuses anything else.
- `tests/plugin.rs`: the plugin, run by Node.js, forwards what it must, in order, to a fake
  bridge.
- `tests/plugin_conformance.rs`: ignored native OpenCode 1.19.5 capture through the real
  launch, Bun plugin and built bridge, using a local model and fresh HOME/XDG without
  account access or package installation (#417). Proves a completed read, an authenticated
  permission refusal and an aborted turn without an outcome. Recorded events and bridge
  reports replay in ordinary CI; exact version/digest, reproduction and remaining
  schema-only cases are in the [fixture provenance](tests/fixtures/opencode-1.19.5/README.md).
  The capture exposed #516: a trailing busy event clears the persisted completed outcome.
  Native capture is proven; owner-visible completion persistence remains open.
- `tests/server.rs`: the server API and provider contract against a fake OpenCode server on
  the loopback, including literal inbox delivery, commands and fail-closed boundaries.
- `tests/discover.rs`: finding `opencode` and its version.
- `tests/conformance.rs` (ignored without an installed OpenCode): the installed version is
  the pinned one, its `--help` documents every flag the adapter passes, the binary reads
  every variable it sets, and a launched server answers its password and lists commands.
