# Each OpenCode session runs its own password-protected server, and a plugin reports its events

Issue #47. Decided by the worker, from OpenCode 1.19.5's source and documentation.

- **One server per session, launched with the TUI.** `opencode --port <free loopback port>
  --hostname 127.0.0.1` with `OPENCODE_SERVER_PASSWORD` minted per launch. The TUI's own
  server is the session's server, so nothing else needs starting or stopping, and no local
  program reaches it without the password. `Server::from_launch` reads both back from the
  launch. Rejected: one shared `opencode serve` (one failure stops every session, and a
  session could drive another's), and a port without a password (any local user could run
  commands as the owner).
- **A new conversation launches unnamed.** OpenCode 1.19.5 creates its session at the first
  prompt and refuses a `--session` it does not know (`validateSession`), so the adapter
  cannot mint one; the bridge's first report names it, as `cockpit-sessions` already allows.
- **The plugin loads through `OPENCODE_CONFIG_CONTENT`** with an absolute path, merged over
  the owner's configuration, and reports only in the server whose port its launch set
  (`COCKPIT_OPENCODE_PORT`): an `opencode` a tool starts inherits the environment, not the
  port. An `OPENCODE_CONFIG_CONTENT` the owner already set is kept with the plugin added to
  its list; one that is not a JSON object refuses the launch rather than being dropped. Rejected: writing into `~/.config/opencode` or the project's `.opencode/` (changes the
  owner's files and loads the plugin in every OpenCode).
- **The plugin filters, Rust decides.** It forwards a fixed list of event forms, in order, to
  `cockpit-opencode-bridge`; normalization lives in `events.rs` with its tests. It drops
  subagents' child sessions, except their permission and question dialogs, which wait in the
  same terminal and go under the root session, and forwards an ended assistant message once.
- **A turn's outcome comes from the assistant message that ends it**, by OpenCode's own rule
  (`finish` not `tool-calls`/`unknown`); its `error` name is the failure reason. An
  `APIError` is `failed`, never `quota-stopped`, as with Claude (#212). A turn the owner
  interrupted (`MessageAbortedError`) did not fail: it reports waiting with no outcome.
- **Submit and cancel need the bridge.** `input` acts only on a known activity, and without
  the plugin's events none is ever known.
- **Effort is not reported.** The TUI sends the variant it saved for the model with every
  prompt, so no launch flag or configuration sets it reliably; variants go with messages the
  server delivers (`Server::deliver`, `Server::invoke_command`, `Server::variants`).
- **Commands are served, not reported.** `Server::commands` and `Server::invoke_command`
  exist; the `commands` capability waits for its contract method, like `cockpit-claude`.
- **A minimal HTTP/1.1 client on the standard library** (`src/http.rs`): loopback only, one
  request per connection. Rejected: an HTTP client crate, for five JSON requests to a local
  server.
- **The pinned events are written from 1.19.5's schemas**, not captured live: this session
  could not run OpenCode. `tests/conformance.rs` (ignored) checks the installed binary and
  passed against the official 1.19.5 release, run on Theo's machine; capturing live events
  stays an open `validate` item.
