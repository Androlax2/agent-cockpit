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
- `torlek-sandbox engine-relay SOCKET OWNER_UID [EGRESS_PORT]`: run by a project engine's
  relay unit (#390) in the engine's network namespace; serves only `OWNER_UID`, which names
  one port per connection and gets that port's bytes, or asks for the engine's egress
  listener (`engine_relay.rs`, below). Without `EGRESS_PORT`, as an earlier setup's unit
  runs it, the egress listener is refused, naming the setup to run again.
- `torlek-sandbox engine-proxy RELAY_SOCKET ENGINE_UID PROJECT FOLDER EGRESS_PORT`: the egress
  proxy of a project's engine (#390), run by the owner's daemon for each recorded engine;
  serves the listener the relay hands it until the relay's connection or the daemon's pipe on
  its standard input ends, and prints `serving` once it holds it (`engine_proxy.rs`, below).
- `torlek-sandbox engine-masks ENGINES_DIR`: the root helper of the project engines (#390),
  run once per connection with the connection as its standard input; it masks one checkout
  per request for the engine the request names, answering only that engine's owner (below).
- `torlek-sandbox run-egress FOLDER SESSION RUN HOST...` (#1387): an approved run's proxy,
  started by the daemon on the host's network before the run. It opens the hand-off socket
  in `FOLDER/proxy-handoff`, prints `ready`, takes the listener the run hands over and
  serves it with the card's hosts as its only entries (`cockpit_sandbox::card_allowlist`),
  filing no network request. When its standard input closes it prints `refused DEST` for
  each destination it refused, then exits (`run_egress.rs`).
- `torlek-sandbox hand-over SOCKET PROGRAM ARGUMENT...` (#1387): the first program of an
  approved run's service. It binds `127.0.0.1:3128` in the service's network namespace,
  sends the listener to `run-egress` on `SOCKET` (stage 2's hand-off, `handoff.rs`) and
  runs `PROGRAM` in its place, keeping no copy.
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

### The project engine's socket

Issue #390. The spec's `engine` (`cockpit_sandbox::EngineSocket`) names the project engine
the daemon verified: its user, uid and Docker socket. Before bubblewrap starts, the launcher
connects to that socket and refuses the launch unless its peer is the engine's uid
(`proxy::check_engine_socket`); bubblewrap then binds the socket read-only, alone in its
folder, at `cockpit_sandbox::ENGINE_SOCKET_INSIDE`, stage 2 lets the program connect there,
and `DOCKER_HOST` names it. A spec for a check run, or an engine run as root, is refused.

### The project engine's egress

Issue #390, option A (Theo, 2026-11-14). The engine's network namespace has no route out;
dockerd, and the containers and builds a session's Docker client points at it, reach slirp4netns's
`10.0.2.2`, which is that namespace's `127.0.0.1`. `engine-proxy` connects to the relay's socket, refuses it unless its peer is
`ENGINE_UID`, and asks `egress`. The relay listens on `127.0.0.1:EGRESS_PORT` in the engine's
namespace and sends the listener over (`handoff::LISTENER_BYTE`), keeping no copy, then holds
the connection: the port is open exactly while that proxy runs, a second proxy is refused
while the first holds it, and the proxy exits when the relay ends. The proxy checks the
listener is on `127.0.0.1:EGRESS_PORT`, then serves it as a session's proxy for the subject
`Subject::Containers`, with no allowlist of its own: only the project's "allow always"
decisions pass (#856 Q4 A); an unknown host is refused at once and filed in `FOLDER`'s
requests file, beside the decisions and counters files the daemon shares with it. A
`FOLDER` that is no folder is refused before the listener is asked for
(`tests/engine_proxy.rs`).

### The project engine's masks

Issue #390, option C (Theo, 2026-11-13). The engine is to see the project's checkouts only
through the masks a session gets: `cockpit_sandbox::checkout_masks`, planned by the same
`plan::protect` and `plan::hide` that build bubblewrap's arguments. `engine-masks` shows it
one checkout per request, a clone made after the engine started included, without a root
step each time. `torlek-engine-masks.socket` runs it, one `torlek-engine-masks@.service` per
connection (`cockpit-containers`' `units/`); a package installs them, or
`torlek setup engine-masks` does.

Each engine's setup writes `ENGINES_DIR/<user>.conf` (`uid=`, `owner_uid=`,
`project_root=`, `clones_dir=`, each once; neither folder inside the other). The request is
the engine's user, a space, a checkout's path and a newline; the answer `ok` or
`refused:`, a space and why, and a refusal leaves the helper's exit status 0, its reason in the
journal. The helper answers only the owner's uid (`SO_PEERCRED`); anyone else, or a missing
or unreadable file, is told only that no such engine is set up for them. It masks only
`project_root` or a folder directly in `clones_dir` that it reaches without a link, the
owner's, holding a `.git`, no mount and on no FUSE file system.

Before it touches the checkout it checks the unit: its main process (`systemctl show`) runs
as the engine's uid, in the helper's user namespace, with a mount namespace of its own that
shows neither `clones_dir` nor the folder holding `project_root` as the host has them. It
then builds the masked checkout in a mount namespace of its own: the checkout, checked
again, is bound in a folder only that namespace has, and each mask is planned there and
mounted on a file opened without following a link; it checks each mask sits on the file it
planned for, clones the tree (`open_tree`), enters the unit's mount namespace and moves the
tree there, in place of an earlier one. The unit's mounts propagate down to rootlesskit's
user namespace, where they arrive locked: root there, as a privileged container is, cannot
unmount a mask, make one writable or bind the checkout without its masks
(`tests/engine_masks.rs`, which stands in for root and the engine with user namespaces). An
environment file made after a request stays readable to the engine until the next one.

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
