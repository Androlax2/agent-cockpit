# A refused host becomes a network request the owner allows once, allows always or denies

Issue #674 (supersedes #641). Decided by Theo for the verbs, the TUI keys and the meaning of
"allow once"; the channel, the grace and the storage by the worker, for the reasons below.

- **"Allow once" lets the host through for the rest of the asking session, not for one
  connection.** Theo decided it (following #860), although the issue text said one
  connection: a client retries, reconnects and pools, so a one-connection allow would ask
  again within seconds. The decision ends with the session; a new session asks again.
- **"Allow always" is a revocable grant to the project for that host and port only**
  (`sandbox_network_grants`, D4 of #546). It reaches every session of the project, survives
  restarts, and `torlek network revoke` ends it: the launcher closes every open tunnel the
  grant carried within its decisions poll.
- **The owner decides on the host-only socket alone.** `torlek network list | allow ID
  [--always] | deny ID | grants [PROJECT] | revoke GRANT`; `approval list` shows the asking
  requests too; the TUI pane takes `y` (allow once), `a` (allow always) and `x` (deny). Every
  network verb is refused on the agents' socket. Requests only ask: no path adds an entry
  without a decision (test `no_network_verb_on_the_agent_socket_allows_a_host` in
  `crates/cockpit-daemon/tests/sandbox.rs`).
- **The launcher and the daemon talk through two files in the session's mount folder**, which
  no agent reaches: `egress-requests.json` (the launcher lists what it refused for want of an
  entry, at most 256 destinations) and `egress-decisions.json` (the daemon writes the owner's
  decisions, replaced atomically; the launcher reads it every 200 ms and before its first
  connection). The launcher holds no daemon socket, and a missing or unreadable decisions file
  means no decisions: it fails closed. The daemon reads the requests every 250 ms and rewrites
  the decisions at its start and after each decision. Code:
  `crates/cockpit-sandbox-launcher/src/proxy.rs`, `crates/cockpit-daemon/src/network.rs`.
- **The refusal is still immediate.** The proxy answers `403` at once, adding "the owner is
  asked: `torlek network list` shows the request, `torlek network allow` lets it through", or,
  once denied, "the owner denied network request N". Nothing waits for the owner.
- **Only DNS names are asked about.** IP literals and names resolving to forbidden addresses
  stay refused without a request; the daemon files a destination only for a live session, as
  a lower-case name of at least two labels.
- **An allow within 2 s of the request appearing is refused** (`NETWORK_ALLOW_GRACE_SECONDS`),
  as for run requests (#156): a keystroke meant for what the queue showed before allows
  nothing. The TUI pane's own grace already outlasts it.
- **The rows live in `cockpit-sandbox`, migration 2 of its set**, beside the evidence, so no
  crate gains a dependency; rows are never deleted and record who decided, when and through
  which client (#657).
- **The wire shape is stable for the phone (#245):** `NetworkRequestView` names its subject
  (kind and id), project, kind (`host`), target and state; a change shows as the approvals
  view stamp (`ViewArea::Approvals`), after which a client sends `NetworkList`.
- **Rejected:** a daemon socket in the launcher, which would give the sandbox's side a
  channel to the daemon; holding the connection open until the owner answers, which the issue
  rules out; adding network requests to `ApprovalQueue`'s reply, which every client of the
  existing queue would have to change.
