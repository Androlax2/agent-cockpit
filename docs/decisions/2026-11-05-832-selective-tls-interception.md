# A sandboxed session's proxy adds the project's GitHub API token on GitHub's hosts, through a per-session CA, and the token never enters the sandbox

Issue #832. Decided by Theo (#546 D1 stage C, D2; #857 decisions 1 and 3; the crates: rustls
0.23 with ring, rcgen 0.14 with ring, httparse 1, pinned exactly, and the host's CA bundle for
upstream servers, 2026-11-05) and by the worker (the rest, reasons below).

- **The CA lives in the launcher's memory for one launch.** `torlek-sandbox` generates an ECDSA
  P-256 CA at each launch, issues one certificate per bound host and keeps only the server
  configurations; the CA's key is dropped once they are issued and is never written
  (`crates/cockpit-sandbox-launcher/src/authority.rs`). The CA is name-constrained to the
  bound hosts and may sign no intermediate (path length 0), so a client that trusted it
  everywhere would still accept it for those hosts alone. Validity runs from the day before
  the launch for 366 days.
- **Trust variables per runtime, in the session's read-only configuration folder.**
  `SSL_CERT_FILE`, `GIT_SSL_CAINFO`, `REQUESTS_CA_BUNDLE` and `CURL_CA_BUNDLE` name a bundle
  of the host's roots then the session CA, since each replaces its client's roots;
  `NODE_EXTRA_CA_CERTS` names the CA alone, which Node adds to its own. Checked by hand on
  this host (2026-11-05) through the proxy: curl 8.23 (OpenSSL 3.6), gh 2.102 (Go), git 2.56,
  node 26.13 (`NODE_USE_ENV_PROXY=1`) and python 3 `urllib` each accepted the
  name-constrained CA and got the credential added.
- **The value travels in its own owner-only file, not on an inherited descriptor.**
  The #857 report named an inherited descriptor; the launch runs through tmux, which passes
  no descriptor to the pane's program. The daemon writes `credentials.json` (mode `0600`)
  beside the spec in the session's mount folder, which no agent reaches; the launcher reads
  and removes it before anything else, as it does the spec (#434). A file beside a spec that
  binds nothing is removed and refused. The spec names credentials and hosts, never a value.
- **Two tokens per project.** `github-tokens/<project>` (#754) stays the push token: an
  unsandboxed agent's `GH_TOKEN`, never a sandboxed one's (#857 decision 1, pushes go through
  `torlek push`). `github-api-tokens/<project>` is the API token (Pull requests and Issues
  write, Contents read), read under the same rules by `cockpit_config::read_github_token`,
  and only for a sandboxed launch: an unsandboxed one has no proxy to add it.
  Under `agents.sandbox = "required"` the proxy adds it as Bearer on `api.github.com` and as
  Basic `x-access-token:<token>` on `github.com` for `git-upload-pack` alone; anything naming
  `git-receive-pack` (both halves of a push, after percent-decoding) is answered 403 naming
  `torlek push`; other requests on `github.com` pass unchanged. The agent's `GH_TOKEN` holds a
  placeholder so `gh` runs.
- **A binding never grants reach (#857 decision 3).** The spec refuses a credential bound to a
  host its allowlist does not let through on 443. The daemon binds the API token to the
  GitHub hosts the launch already reaches and names each other one in the evidence ("is not
  added on github.com:443: the session may not reach it"), rather than refusing every launch
  of a project that has the file. Today no adapter declares GitHub's hosts, so the token is
  bound once the owner's grants (#674) let the session reach them.
- **Only the bound hosts are intercepted.** Every other host stays the plain tunnel of #831.
  A client that rejects the session CA (pinned, or trusting neither variable) ends the
  handshake with an alert; the proxy counts it (`ca_refused`) and names the host and the
  credential in the report's last reason.
- **One HTTP/1.1 exchange per tunnel, framing checked.** ALPN offers `http/1.1` alone on both
  sides. Both heads say `Connection: close`, and the client opens a new tunnel for its next
  request: a kept-alive upstream connection the server closed while idle would fail the next
  request with nothing sent back, and a client cannot retry a `POST` that met the close. A
  request with both `Transfer-Encoding` and `Content-Length`, differing lengths, or a last
  coding other than `chunked` is refused 400 (request smuggling); a response framed that way
  ends the tunnel before its head reaches the client. `Upgrade` requests are refused, and a
  `101` ends the tunnel: a WebSocket or HTTP/2 over a credential host is not carried. The
  request's `Host` (or absolute-form authority) must name the tunnel's host. Each side's read
  waits at most the idle timeout (300 s). `Expect: 100-continue` is answered by the proxy.
- **The credentials file never outlives its launch.** The launcher removes it before it parses
  the spec, so a spec it refuses leaves none behind; the daemon removes it when the launch
  returns, so a launch that failed before its launcher ran, or a launcher too old to know the
  file, leaves none either.
- **Upstream servers are verified against the host's CA bundle.** The owner's `SSL_CERT_FILE`
  when the daemon's environment sets one, else the first of the usual distribution bundles;
  no bundle refuses the launch. A failed upstream handshake is a 502 naming the TLS error.
- **Rejected:** a CA kept on disk across launches (its key would outlive the session);
  intercepting every allowed host (the issue intercepts credential hosts alone, and other
  hosts' pinning would break).
- **Residual:** byte counters of an intercepted tunnel count the plaintext; Git LFS requests
  get no credential, and WebSockets on a credential host are refused; an upstream that answers
  a request expecting `100 Continue` early and closes is seen as a broken tunnel, not
  relayed; whether `gh pr merge --auto` works with the API token's permission set is to be
  tested on a real repository (#857). Provider sign-ins (#858) and registry tokens (#859) are
  not bound yet.
