# cockpit-sandbox-launcher

torlek-sandbox: starts one agent inside bubblewrap's namespaces, then applies Landlock and a
seccomp filter from memory before running it (#829).

Decisions D3 and D5 of [#546's report](../../docs/research/2026-11-05-546-own-sandbox-and-traffic-control.md):
bubblewrap is required and builds the namespaces and the mount view; this program applies
Landlock and seccomp itself, with the `landlock` and `seccompiler` crates pinned exactly and
`unsafe_code = "deny"` kept. It ships in `bin/` beside the daemon, which probes it at its
start and runs it in the agent's place under `agents.sandbox = "required"`.

## Verbs

- `torlek-sandbox run SPEC`: reads and removes the daemon's launch spec
  (`cockpit_sandbox::LaunchSpec`, written owner-only in the session's mount-source folder),
  validates it, then runs bubblewrap with new user, mount, PID, network, IPC, UTS and cgroup
  namespaces and a mount view of only what the spec names, and waits. The program's exit
  status is the launcher's; 125 means the launcher refused or failed before the program ran.
- `torlek-sandbox stage2 POLICY ARGUMENT...` (internal): inside the namespaces, checks that each folder
  the plan pinned is still the one it saw (device and inode), applies Landlock at the probed
  ABI with `HardRequirement`, installs the seccomp filters, then runs the program in its
  place.
- `torlek-sandbox stage2-binder` (internal): started by stage 2 once Landlock and seccomp
  hold, on the hand-off socket. It binds the sandbox's `127.0.0.1` ports the launcher asks for
  (a live container port, below) and hands each listener back; the program keeps no
  descriptor to it.
- `torlek-sandbox engine-relay SOCKET OWNER_UID`: run by a project engine's relay unit (#390)
  in the engine's network namespace; serves only `OWNER_UID`, which names one port per
  connection and gets that port's bytes (`engine_relay.rs`).
- `torlek-sandbox probe BWRAP`: the daemon's start-up probe. Prints
  `{"bubblewrap":"<version>","landlock_abi":<n>}`; a failure is the reason every launch under
  `required` is refused.

## Mount view

System folders (`cockpit_sandbox::SYSTEM_READ_ONLY`) read-only; the session's own home at the
owner's `HOME` path; the checkout, the session's folder and its temporary folder read-write;
the bridge programs, the instruction bundle and the agent socket's folder read-only; a fresh
`/tmp`, `/proc` and `/dev`. Inside the checkout, `.git/hooks`, `.git/config`, `.torlek` and
`.cockpit` are read-only (an empty read-only folder when absent), and so is a `.git` file
(a worktree's or submodule's); `.git/modules`, `.envrc` and the editors' task files are
read-only when present; `.env` files and the host's denied paths are masked, a link's
target with it. The terminal or file each standard stream already is can be reopened
through `/dev/stdout` and `/dev/stderr`.

## Session proxy

Decisions D1 and D2 of #546's report, issue #831. The network namespace is empty but for
`lo`. Before Landlock, stage 2 listens on `127.0.0.1:3128` there and hands the listener over
a Unix socket in a launcher-owned folder (`handoff.rs`); the launcher serves it from the
host's network, one thread per connection (`proxy.rs`). Clients find it through
`HTTPS_PROXY` and its siblings; nothing inside resolves names, and no UDP leaves.

- Only `CONNECT`; plain HTTP is refused.
- The decision is `cockpit_sandbox::Allowlist` (the spec's `egress`): the adapter's hosts
  and, when it declares any, the checkout's ecosystem registries, each naming its source;
  the owner's decisions on network requests join them (#674). An
  unknown host gets `403` at once, with the reason, the session and a `Proxy-Status` that
  separates a refusal (`http_request_denied`, `destination_ip_prohibited`) from an
  unreachable upstream (`dns_error`, `destination_unavailable`, `connection_timeout`).
- A name refused for want of an entry is listed in `egress-requests.json` beside the spec,
  and the refusal says the owner is asked, naming the request once the daemon filed it; the
  status line carries the same sentence (#966). The daemon files it as a network request; the
  owner allows it for the rest of the session, allows it always for the project (a grant),
  or denies it. The decisions arrive in `egress-decisions.json`, read before the first
  connection and every 200 ms; a missing or unreadable file means none. A revoked decision
  closes the tunnels it carried. See `docs/decisions/2026-11-05-674-network-requests.md`.
- IP literals and names resolving to loopback, private, link-local, metadata or other
  non-public addresses are refused unless an entry grants that address (no owner decision
  does: only names are asked about; a host-service forward, #880, is never one); one such address
  among a name's answers refuses the connection, and the proxy connects only to the
  addresses it checked.
- On port 443 the ClientHello's server name must match the `CONNECT` host; a missing or
  different name closes the tunnel. Any other granted port tunnels without a peek.
- The counters (`cockpit_sandbox::EgressReport`) go to `egress.json` in the session's
  mount-source folder, at most once a second and at exit; `torlek session sandbox` shows
  them under the evidence.

## Host-service forwards

Issue #880, from [#860's report](../../docs/research/2026-11-05-860-host-service-forwards.md).
Each of the spec's `forwards` is a declared service: a `listen` inside the sandbox and a
`target` on the host, each a loopback port or a Unix socket (`cockpit_sandbox::Forward`). A
port listen is bound by stage 2 on the sandbox's `127.0.0.1` and handed over with the proxy
listener; a port below 1024, or one another forward or the proxy holds, refuses the launch
naming the service. A socket listen is created by the launcher in a folder of its own under
`forward-sockets/` beside the spec, bound read-only at the listen path's folder, so the
program can neither remove the socket nor add one; a listen folder overlapping a folder the
view binds or another listen's folder is refused, as is a socket path that is not UTF-8 or
holds a space or control character, which would make two records read alike
(`proxy/forward.rs`). A socket target is connected within the same 10 seconds as a port
target.

- A connection is relayed only while the owner's decisions allow the forward's whole record
  (`Forward::record`: name, listen and target). Otherwise it is closed at once, and the
  record is listed under `services` in `egress-requests.json`; a denied record is closed
  naming the request and asks no more.
- The relay copies bytes both ways and parses nothing; a revoked decision closes the relays
  it allowed. The proxy's loopback refusal is unchanged: a forward is never a `CONNECT`.
- Each service's counters (connections, bytes, refusals, unreachable targets) are under
  `forwards` in `egress.json`.

### Live container ports

Issue #390. A container port opens while the session runs, with nothing in the spec: the
owner's decisions carry it as `AllowedService::live`, a forward of the `containers` service
from the sandbox's `127.0.0.1:P` to `engine:USER:P` (`Forward::container_port`, P at least
1024 and not the proxy's), and `ServiceDecisions::engine` names the engine's recorded user,
uid and relay socket. At each decisions change the launcher asks the binder for each newly
allowed port, checks the listener it gets back is on the sandbox's loopback at that port,
and serves it like a declared forward; a refused bind is recorded as the service's last
reason. A connection goes to the engine's relay socket only when the socket's peer is the
engine's uid, then names the port (`engine_relay::request`). A revoked port's listener is
shut and its relays closed; a connection reaching it meanwhile is closed without filing a
request.

## Credential hosts

Issue #832, D1 stage C. When the spec's `interception` binds credentials, the launcher reads
and removes `credentials.json` beside the spec, generates a CA in memory name-constrained to
the bound hosts, issues one certificate per host, drops the CA's key and writes the CA's
certificate and the host's bundle with it into the session's read-only tool configuration
folder (`authority.rs`). On a bound host the proxy completes TLS with the client itself,
opens TLS to the upstream server verified against the host's CA bundle, and relays HTTP/1.1
one exchange per tunnel, adding the credential (`intercept.rs`): Bearer on every request, or
Basic on Git fetches alone with any `git-receive-pack` request refused naming the owner route (`torlek ask run`).
Every other host stays a plain tunnel. A client that refuses the session CA is counted
(`ca_refused`) and named with the host in the report. See
[the decision](../../docs/decisions/2026-11-05-832-selective-tls-interception.md).

## seccomp

A deny list, built in memory: tracing, mounts and the new mount API, namespace creation
(`clone3` gets `ENOSYS` so callers fall back to `clone`, whose namespace flags are refused),
kernel modules, keys, BPF, `io_uring`, `TIOCSTI`/`TIOCLINUX`, and socket families other than
Unix, IPv4, IPv6 and netlink. On x86_64 the x32 aliases are refused too.
