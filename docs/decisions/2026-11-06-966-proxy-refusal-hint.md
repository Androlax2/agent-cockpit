# A proxy refusal names the host and the request on its status line, and the brief says how to read it

Issue #966. Decided by the worker: the issue lists three ways and asks the pull request to
pick; this combines the two that need no new channel into the session.

- **The status line carries the reason after `403 Forbidden: torlek-sandbox: `.** Python's
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
