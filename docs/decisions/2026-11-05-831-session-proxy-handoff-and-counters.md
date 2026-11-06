# The session proxy's listener is handed out of the sandbox, and its counters are a file

Issue #831. The descriptor hand-off decided by Theo (2026-11-05, Q1 A: `rustix` `=1.1.6`
with `net` and `std` in `cockpit-sandbox-launcher`); the rest decided by the worker, as
the smallest change that meets the issue. The #966 section below adds how a refusal reads on the
status line (the host and the request, and the brief's rule 3).

- **Stage 2 binds `127.0.0.1:3128` in the empty network namespace and hands the listener
  to the launcher over `SCM_RIGHTS`** (`crates/cockpit-sandbox-launcher/src/handoff.rs`).
  The agent connects to its own loopback and the launcher serves from the host's network,
  so no host interface, resolver or UDP path exists inside.
  **Rejected:** a relay inside the sandbox, because it would be a second program with
  network reach running beside the agent.
- **Only `CONNECT` is served; plain HTTP gets `403`.** Every allowed destination is TLS
  on 443 or a granted port tunnelled as is; serving plain HTTP would mean parsing and
  forwarding requests for no current need.
- **Registry hosts of a detected ecosystem join only when the provider declares hosts**
  (#859's "for a session whose provider declares them"). A program declaring no hosts
  keeps an empty list, so its `403` says "`<program>` declares no hosts".
- **The counters are `egress.json` in the session's mount-source folder**, written by the
  launcher (staged file and rename) at most once a second and at its exit, and read by
  `Sandbox::egress_report` into the `torlek session sandbox` evidence. The launcher holds
  no daemon socket. The report keeps 256 destinations and counts the rest under `other`.
  **Rejected:** a channel to the daemon, because it needs a new authenticated request for
  data the daemon only shows on demand.

## Revisited 2026-11-06 (#966)

Decided by the worker: the issue lists three ways and asks the pull request to pick; this
combines the two that need no new channel into the session. A proxy refusal names the host and
the request on its status line, and the brief says how to read it.

- **The status line carries the reason after its reason phrase.** Python's
  `http.client` (and pip, Go) print a failed `CONNECT`'s status line, as `Tunnel connection
  failed: 403 Forbidden: torlek-sandbox: HOST is not allowed: ...`, and `curl -v` shows it on
  its `< HTTP/1.1` line; no client prints the body. curl and git without `-v` print only the
  code. The text is the `Proxy-Status` details' printable ASCII, both cut at 512 bytes, so a
  refusal of the longest DNS name still ends with the owner's command. The status code, the
  `Proxy-Status` error and the decision are unchanged. Code: `answer` in
  `crates/cockpit-sandbox-launcher/src/proxy.rs`.
- **The refusal names the request once the daemon filed it.** `egress-decisions.json` also
  lists the requests still asking (`EgressDecisions::asking`), which allow nothing, and the
  daemon rewrites the file right after filing one. The refusal then says "the owner is asked
  in network request N: `torlek network allow N` lets it through"; the first refusal, before
  the daemon read the requests file, keeps the old wording. Test:
  `a_retry_names_the_filed_request_and_the_owners_command_on_its_status_line` in
  `crates/cockpit-daemon/tests/sandbox.rs`.
- **The launch brief's rule 3 says a proxy `403` is no command to ask for** and points to
  `curl -v`, whose next try names the request, and to `torlek docs sandbox`. curl and git
  print only `CONNECT tunnel failed, response 403`, so the agent learns this from the brief,
  not the refusal.
- **Rejected: waiting for the request number before answering.** The refusal stays
  immediate (#674, #831); the number comes on the next try, about a second later.
- **Rejected: a Torlek note typed into the session when a request is filed.** It needs a
  daemon-authored message path that coordination messages do not have (they are audited per
  sending session and grant), and a note lands after the turn that hit the refusal.
