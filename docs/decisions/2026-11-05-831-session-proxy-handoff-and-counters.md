# The session proxy's listener is handed out of the sandbox, and its counters are a file

Issue #831. The descriptor hand-off decided by Theo (2026-11-05, Q1 A: `rustix` `=1.1.6`
with `net` and `std` in `cockpit-sandbox-launcher`); the rest decided by the worker, as
the smallest change that meets the issue.

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
