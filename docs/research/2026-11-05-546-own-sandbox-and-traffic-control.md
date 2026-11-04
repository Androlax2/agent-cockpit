# Torlek's own sandbox and agent traffic control

Issue [#546](https://github.com/Androlax2/agent-cockpit/issues/546), re-scoped by the owner on
2026-11-05: Torlek replaces Greywall and Greyproxy with its own sandbox and network traffic
control. **Research and proposal, 2026-11-05; Theo's decisions are recorded under
[Decisions for the owner](#decisions-for-the-owner) as they are made.** Merging this document
accepts research delivery only. It authorizes no product code, dependency, host setting,
engine removal or follow-up issue; the issues proposed at the end are not filed. The earlier
[repository-placement report](2026-11-02-546-engine-repositories.md) answered the previous
question (import the Go engines or keep them separate) and is superseded by this direction.

## Recommendation and decisions

Build the replacement for Linux and WSL2 first, from five pieces Torlek owns:

1. **A session launcher, `torlek-sandbox`.** One small Rust program per session, started where
   Greywall starts today. It builds the agent's namespaces (user, mount, PID, network, IPC,
   UTS), applies Landlock and a seccomp filter from memory, and runs the provider. It reads its
   policy from the daemon over an inherited file descriptor: no settings file (#434), no seccomp
   file in a shared folder (#628).
2. **No network in the agent's namespace except one loopback listener.** The launcher creates
   the listener inside the namespace and keeps serving it from outside (file-descriptor
   passing; prototype measured below). Agents reach the network only through it, with
   `HTTPS_PROXY`/`HTTP_PROXY`. Anything that ignores the variables fails closed.
3. **A per-session egress proxy in that launcher**, not a shared service. It decides on the
   `CONNECT` host and port and checks the TLS SNI against it, resolves names itself, refuses
   private and loopback addresses unless granted, and refuses an unknown host at once with a
   reason. The refusal becomes an owner decision in Torlek (#674): allow once, allow always
   for the project, deny. No fixed ports, no API token, no client secrets, no dashboard.
4. **TLS interception only for hosts that carry a credential, and second** (D1, decided C,
   staged). The proxy ships first without interception: no CA. The next issue adds selective
   interception for hosts with a bound credential, through a per-session CA whose key never
   leaves the launcher's memory, so a token such as `GH_TOKEN` never enters the sandbox.
5. **Policy from Torlek's own records.** Provider hosts and paths move into the provider
   adapters; network allow rules become audited, revocable grants; the launcher reports what it
   actually enforced.

Migrate behind one owner setting, run the same live enforcement suite (#433) and red-team
(#685) on both backends, make the native one the default, then delete the engines.

Performance does not decide this. On the research host a native launch prototype costs about
0.75 ms over a plain start (bubblewrap: about 1.8 ms), and each loopback path tested adds
0.05 to 0.42 ms per request ([measurements](#what-was-measured)); a provider API call takes
hundreds of milliseconds. The case rests on correctness, one lifecycle and owner control.

- [x] D1 decided 2026-11-05: C, staged (A first, then selective interception).
- [x] D2 decided 2026-11-05: A, the proxy runs in each session's launcher.
- [x] D3 decided 2026-11-05: A, bubblewrap as a required host program; Torlek applies Landlock
  and seccomp.
- [x] D4 decided 2026-11-05: A, "allow always" covers the project only, as a revocable grant;
  #674's text changes from the whole profile to the project.
- [x] D5 decided 2026-11-05: A, the `landlock` crate and `seccompiler`, pinned exactly; the
  workspace keeps `unsafe_code = "deny"`.
- [x] D6 decided 2026-11-05: A, stages 1 to 4 in v0.1.0-alpha.4, stage 5 in v0.1.0-alpha.5,
  macOS in Roadmap: macOS.
- [ ] Theo approves or declines each
  [proposed issue](#proposed-issues-not-filed).

## What was measured

Host: AMD Ryzen 7 7800X3D (16 threads), Arch Linux kernel 7.2.14, bubblewrap 0.13.1, Landlock
ABI 10, active LSMs `capability,landlock,lockdown,yama,bpf`. The research session ran with the
profile's `agents.sandbox = "off"`, so it could create namespaces itself. Method, scripts and
raw output: [appendix](546/measurements.md).

| What | Result |
| --- | --- |
| Start `/usr/bin/true`, 300 runs, median | 254 to 272 µs |
| Same in new user, mount, network, IPC and UTS namespaces (`unshare`) | 969 to 1021 µs |
| Same under bubblewrap (`--unshare-all`, minimal binds) | 2035 to 2037 µs |
| Native prototype: namespaces + Landlock (filesystem, TCP, scopes) + seccomp, then `exec` | 1020 to 1031 µs |
| Small HTTP request on loopback, direct (ab, 2000 requests, no keep-alive) | 0.252 to 0.296 ms |
| Same from an empty namespace via two `socat` processes and a Unix socket (sandbox-runtime's model) | 0.662 to 0.669 ms |
| Same through slirp4netns | 0.296 to 0.297 ms |
| Same through a listener created inside the namespace and served outside by fd passing (Python relay) | 0.379 to 0.410 ms |
| Bulk download, 256 MiB, MiB/s: direct / slirp4netns / two `socat` | 3,433 to 3,659 / 2,450 to 2,598 / 1,580 to 1,704 |
| Same through the fd-passed listener | 960 to 4,438 (bounded by the single-threaded test server, not the path) |
| Managed Greyproxy, idle, after 2 h 12 min | 24.6 MB resident, 17 threads |

Not measured: Greywall's own launch time and its TUN path, because the pinned Greywall talks
to the fixed management port of the profile's live Greyproxy, which is never a validation
target. Its per-session helper processes are counted from source instead.

## Inventory: what the engines do for Torlek today

Pins: Greywall [`a9ccc58`](https://github.com/Androlax2/greywall/tree/a9ccc58c4e57f7ad7aaf7c2ab3779d71cf485baf)
(Go, 15,013 lines without tests, Apache-2.0, embeds GPL-3.0 tun2socks 2.5.2) and Greyproxy
[`15ad5fb`](https://github.com/Androlax2/greyproxy/tree/15ad5fbfd582d3d85fce79fec09f5a26bb18323e)
(Go, 49,466 lines without tests of which about 27,500 are a vendored copy of the proxy it forked, MIT, 31 direct
dependencies). Paths below are inside those trees.

### Mechanisms

| Mechanism | What the pinned engines do | What Torlek uses |
| --- | --- | --- |
| Process isolation | bubblewrap with `--unshare-net`, `--unshare-pid`, private `/dev`, `/proc`, tmpfs `/tmp`, `--die-with-parent`; IPC, UTS and cgroup namespaces shared with the host (`internal/sandbox/linux.go`) | All of it, through `cockpit-sandbox`'s argv |
| Filesystem | Empty root, read-only system and toolchain folders, the checkout read-write, then denials applied last as masks; Greyproxy's data folder masked except `ca-cert.pem`; Git metadata and start-up files protected | `defaultDenyRead`, `denyRead`, `denyWrite`, `--allow-path`, `--allow-read-path`, the fork's `--mount` for Codex's sockets |
| Landlock | Applied by `greywall --landlock-apply` inside the sandbox; filesystem rights up to ABI 5; no network rules, no ABI 6 scopes; refuses to start without Landlock | Required capability (`greywall.rs`) |
| seccomp | Hand-built BPF denying 27 syscalls and `ioctl(TIOCSTI)`, written to `$TMPDIR/greywall-seccomp/` and handed to bubblewrap on fd 3 (`linux_seccomp.go`) | Required capability; Torlek probes the folder (#628, #650) |
| Network | In the namespace: `tun0` with a default route, tun2socks to an in-namespace `socat`, a Unix socket, a host `socat`, Greyproxy SOCKS on 43052; DNS through a UDP `socat` to Greyproxy on 43053 | "network: through greyproxy only" |
| Proxy identity | Per-run 32-byte client secret; session registered with `POST /api/sessions` under the management token; 900 s TTL, 60 s heartbeat | A fresh client name per launch (`ClientIdentity::for_launch`) |
| Holds | An unknown destination is held up to 30 s (`plugins/bypass.go:73`), listed only in Greyproxy's dashboard, then refused (#641) | Nothing: the dashboard's notifications are disabled |
| TLS and CA | ECDSA CA generated by `cert generate`; interception on by default for every sniffed TLS connection; request and conversation capture | Profile-local CA, never in the OS trust store; capture disabled |
| Credentials | ~100 variable names and suffixes replaced by placeholders, swapped back in request headers and queries by Greyproxy | Asks for none and passes its launch token through; Greywall's own detection still applies to matching variables |
| Provider needs | 14 built-in agent profiles: paths and hosts (claude: Anthropic, claude.ai, GitHub, npm) | `--profile` (claude, codex or opencode) with `--auto-profile` |
| Desktop | `xdg-dbus-proxy` exposing `org.freedesktop.Notifications` only | Shared desktop sessions (#435) |
| Learning | `--learning` (strace), `--watch`/`greywatch` (allow all), `--monitor` (bpftrace) | Not used; the archive keeps the alias |

Per sandboxed session the host runs Greywall itself (heartbeat), two `socat` bridges (SOCKS and
DNS) and optionally `xdg-dbus-proxy`; inside, tun2socks and two `socat` relays. One Greyproxy per profile
serves every session through fixed loopback ports, so only one Torlek profile per host can
own them.

### Weaknesses found in the pinned engines

Read from source by this research; none was exploited.

1. **Every sandbox reaches the host's loopback services.** Migration rows allow `127.0.0.1`,
   `::1` and `localhost` on every port for every client (`internal/greyproxy/migrations.go:212`);
   the fork's self-guard covers only Greyproxy's own ports. Databases, development servers and
   other daemons on the host loopback are reachable through SOCKS.
2. **DNS is unfiltered:** the DNS handler only caches answers on their way back
   (`internal/greyproxy/plugins/dns_cache_handler.go`) and forwards every query, an open
   tunnel for exfiltration.
3. **Rule matching trusts reverse DNS.** For an IP with no cached forward answer, Greyproxy calls
   `net.LookupAddr` (`internal/greyproxy/dns_cache.go:97`) and matches rules on the answer, which
   the address owner controls.
4. **Placeholders are swapped for any allowed host**, not for the host the credential belongs to
   (`SubstituteRequest`, `internal/greyproxy/credential_store.go:183`).
5. **Silent fail-open paths in Greywall:** Landlock is skipped for a `/tmp` executable or library
   use with a debug message only; a failed rule add is skipped the same way; a failed proxy
   registration leaves real credential values in the environment.
6. **seccomp gaps:** no architecture check; `unshare`, `setns`, new user namespaces, the new
   mount API and `io_uring` are allowed.
7. **Shared fixed paths:** the seccomp filter, mask file and bridge sockets live under `/tmp`
   (#628).

The design below closes each by construction or names the test that must prove it.

### Touch points in `crates/`

| Where | What depends on the engines |
| --- | --- |
| `cockpit-sandbox` | `greywall.rs` (771 non-comment lines: detection, argv, settings file), `install.rs` (504: release download and verification), `proxy.rs` and `proxy/` (1,461: Greyproxy configuration, CA, unit, health, ownership), `seccomp.rs` (89: #628 probe), `engine-pins.json`. Backend-neutral and kept: `profile.rs`, `launch.rs`, `evidence.rs`, `observations.rs`, `platform.rs` |
| `cockpit-daemon` | `serve.rs` detects the engine (`TORLEK_DEV_GREYWALL`/`TORLEK_DEV_GREYPROXY` in `dev`), `readiness.rs`, `sessions.rs` (evidence view), `session_sockets.rs` (`ProviderReach`); tests `sandbox.rs`, `real_daemon/proxy.rs`, `readiness.rs` |
| `cockpit-sessions` | `lifecycle.rs` (`prepare_sandbox`, launch token pass-through, `check_seccomp_filter`), `delivery_context.rs` (the `--mount` feature), `model.rs`, `error.rs` |
| `cockpit-cli` | `sandbox_install.rs` (install, status, restart, stop), `daemon_install.rs` (shared Greyproxy ports), `onboarding.rs` |
| Elsewhere | `cockpit-readiness` (engine versions), `cockpit-protocol` (`client_identity`), `cockpit-config` (`AgentSandbox`, `DesktopSession`); Makefile `sandbox-install-smoke` and `sandbox-enforcement-*`, `tools/verify-sandbox-enforcement.py`; legacy `packages/greywall`, `packages/greyproxy` until the cutover (#66) |

### Issues this changes

| Issue | Milestone | Effect |
| --- | --- | --- |
| [#628](https://github.com/Androlax2/agent-cockpit/issues/628) shared seccomp folder | alpha.4 | Superseded: the filter is loaded from memory. Its two-user acceptance becomes a regression test |
| [#641](https://github.com/Androlax2/agent-cockpit/issues/641) unknown host hangs | alpha.4 | Superseded by #674 |
| [#674](https://github.com/Androlax2/agent-cockpit/issues/674) refuse at once, owner decides | alpha.4 | Kept: built in the native proxy (Theo's 2026-11-03 decision). Text changes: "allow always" covers the project, not the whole profile (D4) |
| [#495](https://github.com/Androlax2/agent-cockpit/issues/495) Greyproxy lifecycle | alpha.4 | Superseded: no shared service. Its fresh-home and "present versus working" acceptance moves to the launcher |
| [#433](https://github.com/Androlax2/agent-cockpit/issues/433) live enforcement CI | alpha.4 | Changed: same probes, run against the native backend built from the same tree |
| [#434](https://github.com/Androlax2/agent-cockpit/issues/434) settings files | alpha.4 | Superseded: policy passes by file descriptor |
| [#482](https://github.com/Androlax2/agent-cockpit/issues/482) mount source cleanup | alpha.4 | Unaffected in substance; the folder stays daemon-owned |
| [#685](https://github.com/Androlax2/agent-cockpit/issues/685) red-team | alpha.4 | Changed: section 1 targets the native sandbox, after it ships |
| [#527](https://github.com/Androlax2/agent-cockpit/issues/527) macOS public CA | Roadmap: macOS | Superseded: the per-session CA's key stays in the launcher's memory (D1) and only its certificate enters the session, so there is no private CA state to protect |
| [#372](https://github.com/Androlax2/agent-cockpit/issues/372) plugin egress | Roadmap: plugins | Changed: shares the native allowlist, decision and audit model |
| [#89](https://github.com/Androlax2/agent-cockpit/issues/89), [#245](https://github.com/Androlax2/agent-cockpit/issues/245), [#691](https://github.com/Androlax2/agent-cockpit/issues/691) | beta.3, beta.4 | Changed: no Greyproxy dashboard; a "hold" is a Torlek network request on the change feed |

Also reworded, not re-scoped: #66 (cutover removes the native-replaced engines), #150 (final
audit covers Torlek-owned code), #568 (macOS needs a native backend), #500 and #501 (remote
hosts need user namespaces and Landlock, not Greywall), #390 (containers use the native
proxy), #59 (its proxy placeholders become credentials bound to hosts by proposed issue 4).
Since #754 (merged in #819) the project's `GH_TOKEN` enters the sandbox: no launch requests a
placeholder, because Git sends the token inside Basic credentials that Greyproxy cannot swap.
Proposed issue 4 takes it out; #754's rule that the sandbox hides `~/.config/gh` applies to any
backend. Unaffected: #155, #352, #716 and #718.

## Threat model for an agent session

Report 11 calls Greywall "defense in depth, not hostile-code containment". The same bar
applies: the adversary is the agent and the code it runs (prompt injection from issues, pages,
dependencies and MCP servers; install scripts; a compromised package), working as the owner's
OS user. A kernel exploit is out of scope; a sandbox escape that needs no kernel bug is not.

| An agent session | Today's mechanism in Torlek | Native mechanism |
| --- | --- | --- |
| **May** read and write its checkout, session folder and temp folder; read system files and its provider's own folders; run builds and tests | Greywall profile + `--allow-path` | Mount view + Landlock |
| **May** reach its provider's API and hosts granted to it | Greyproxy session rules from the engine profile | Per-session proxy, allowlist from the provider adapter and grants |
| **May** talk to Torlek on the agent socket; every verb checked against its grants | Agent channel, `cockpit-authorization` | Unchanged |
| **Must not** read owner credentials (`~/.ssh/id_*`, `~/.gnupg`, cloud and Git credentials, keyrings, `~/.config/gh`, #754) or other projects | Deny masks | Not mounted, and Landlock denies by omission |
| **Must not** persist outside the session: Git hooks and config, `.torlek`, `.envrc`, IDE run configurations, shell start-up, `~/.local/bin`, systemd and autostart units | `denyWrite` | Read-only binds over those paths inside writable trees |
| **Must not** reach Torlek's state, owner socket, logs or configuration, or the proxy's own state | Deny masks | Not mounted; the proxy has no state on disk |
| **Must not** signal, trace or connect to other sessions' or the owner's processes and sockets | PID namespace; ptrace denied; named sockets under read-only folders stay connectable | PID namespace, Landlock signal, abstract-socket and (ABI 9) named-socket rules, seccomp |
| **Must not** reach arbitrary hosts, the host's loopback, the LAN or cloud metadata; tunnel through DNS or UDP | Greyproxy rules (loopback allowed, DNS open) | Empty network namespace; proxy refuses loopback, private, link-local and metadata ranges unless granted; no DNS inside |
| **Must not** start Windows programs or read Windows drives on WSL2 | Windows drives and `/run/WSL` denied, `WSL_INTEROP` dropped | Same rules, unmounted rather than masked |
| **Must not** exhaust the machine | Nothing | Per-session cgroup limits (M8) |
| **Asks the owner** to run a host command, reach a new host, widen a path, share the desktop | `ask run`, `ask guide`; hosts: Greyproxy dashboard only | `ask run`, `ask guide`, network requests in Torlek (#674) |

Residual risks the owner accepts with any design: allowed hosts are exfiltration channels (the
provider API itself, GitHub for a session that pushes); on a host without interception, a
client can send a different HTTP `Host` inside an allowed TLS connection (domain fronting on shared
front ends); code the agent writes runs later wherever it is approved or merged; a same-UID
process outside the sandbox is not contained (#729).

## Options per mechanism

### M1. Namespaces and the filesystem view

Landlock rules only add rights: a path inside a writable tree cannot be made read-only or
unreadable by Landlock alone, so `.git/hooks`, `.git/config` and `.env` inside the checkout need
mount masks. A mount namespace is therefore required, and with it an unprivileged user namespace.

| Option | For | Against |
| --- | --- | --- |
| A. bubblewrap as a required host program (like tmux); the launcher builds its argv, then a second stage inside creates the loopback listener, hands it out over an inherited socket and applies Landlock and seccomp | Mature, widely used setup code (Flatpak); measured 2 ms; the engines already need it; distribution packages carry it | An external program and its version range to check; flags limit what the view can do |
| B. Own Rust setup with the new mount API (`open_tree`, `move_mount`, `mount_setattr`, `pivot_root`) | No external program; exact control (for example idmapped mounts later); 1 ms | Security-critical code Torlek must own and review |
| C. No namespaces: Landlock and seccomp only (landrun's model; Codex used it before moving to bubblewrap) | Works where user namespaces are blocked | Cannot mask files inside the checkout, cannot isolate the network by host |

Ubuntu 24.04 and later let an unconfined program create a user namespace but strip its
capabilities there, so bubblewrap and a Torlek-owned program alike need an AppArmor profile
with a `userns` rule, or `kernel.apparmor_restrict_unprivileged_userns` set to 0: a root step
for the owner (`torlek ask guide`). bubblewrap has had no setuid mode since 0.12.0, so there is
no fallback; the launcher refuses and says why. Debian, Arch's default kernel and WSL2's kernel
allow user namespaces ([sources](546/measurements.md#prior-art-and-platform-sources)).

Decided (D3): A.

### M2. Landlock

Require a floor and report the ABI used; refuse below it, never fall back. The floor is ABI 2
(Linux 5.19): under ABI 1 every rename or link across folders is denied, which breaks build
tools. ABI 4 (6.7) adds TCP `bind`/`connect` rules, ABI 6 (6.12) scopes abstract Unix sockets
and signals to the sandbox, ABI 9 (7.1) adds `RESOLVE_UNIX` for named Unix sockets and
ABI 10 (7.2) UDP. WSL2's current kernel, 6.18, has ABI 7. Landlock's network rules match ports, not addresses. Inside a private network
namespace they add nothing and would break an agent's own test servers on `localhost`, so
the native design leaves TCP and UDP unhandled there and relies on the namespace; port rules
matter only without one (M1 C). The ABI 6 scopes are applied where present but mostly repeat
the namespaces (abstract sockets belong to a network namespace). `RESOLVE_UNIX` adds more: a
named socket the mount view exposes read-only stays unreachable unless allowed. Below ABI 9
the mount view itself must leave out every socket folder the session does not need. The
`landlock` crate (0.4.7) covers ABIs up to 9, all this design uses.

### M3. seccomp

Build the filter in memory and install it in the launcher's second stage; nothing on disk.
Start from Greywall's 27 syscalls and close its gaps: an architecture check, `unshare`,
`setns`, `clone`/`clone3` with namespace flags (`clone3` passes flags by pointer, so it is
denied with `ENOSYS` and callers fall back to `clone`), the new mount API, `io_uring_*`
(its operations bypass seccomp), `ioctl(TIOCSTI)` and `TIOCLINUX`, and socket families other
than Unix, IPv4 and IPv6. Allow-list filters break toolchains on each new syscall; a deny list
with these additions is the proportionate choice. User notification is not needed, and its manual page warns against using it for
security decisions. A
provider's own nested sandbox (for example one built on bubblewrap) cannot start under this
filter; issue 1 checks that each provider is launched with its own sandbox off.

### M4. Network containment

| Option | Non-proxy traffic | Cost | Measured path |
| --- | --- | --- | --- |
| A. Empty network namespace, proxy variables, one loopback listener served from outside (fd passing) | Fails closed: no route, no DNS | No process inside; clients must honor the variables | 0.38 to 0.41 ms per request with a Python relay |
| A'. Same with a forwarder inside (sandbox-runtime's `socat` bridge; Codex's is alike) | Fails closed | One or two forwarders per session | 0.66 ms |
| B. Transparent capture: TUN device and a userspace TCP stack (today's model) | Captured: any TCP; destinations recovered through a fake-IP DNS | A TCP stack to own (or GPL tun2socks); DNS interception; more helpers | Not measured (see above) |
| C. slirp4netns or pasta, restricted by nftables inside the namespace | Routed, then filtered by IP | Another external program; filters by address, not name | 0.30 ms (slirp4netns) |
| D. Landlock TCP port rules only, host network | Any address on the allowed port | None | Not a containment: the proxy port is reachable on every host |

A is the simplest that fails closed. Its cost is compatibility: tools that ignore the proxy
variables fail with a network error instead of being captured (SSH remotes, some language
runtimes). Torlek can set the variables each runtime needs (Node's built-in clients honor them only with
`NODE_USE_ENV_PROXY=1`, since Node 24.0 and 22.21) and add a `CONNECT` helper for SSH later; B stays possible behind the same proxy if a measured need appears.

### M5. The egress decision

Decide per connection on the `CONNECT` host and port (or the absolute URL of plain HTTP), from
the session's allowlist. Resolve names in the proxy, then refuse loopback, private, link-local,
multicast and metadata addresses unless granted by address, which also closes DNS rebinding.
Refuse IP literals unless granted. Peek the TLS ClientHello and refuse when the SNI is missing
or differs from the `CONNECT` host; an Encrypted Client Hello (RFC 9849) usually shows another
outer name and is refused by the same check. No DNS listener exists inside the namespace, so no DNS tunnel.

### M6. TLS interception

| Option | For | Against |
| --- | --- | --- |
| A. None: host and SNI only | No CA, no private key, no trust variables, no certificate pinning failures; #527 vanishes; smallest code (no TLS stack) | Cannot see paths or bodies; cannot keep a credential out of the sandbox; domain fronting inside an allowed host |
| B. Selective: only hosts with a bound credential, through a per-session CA generated in memory and never written to disk | Keeps a token such as a project's `GH_TOKEN` (#754) out of the sandbox; binds each credential to its host | A TLS stack and certificate generation to own; trust variables per runtime; pinned clients break on those hosts |
| C. All traffic (Greyproxy's default) | Full visibility | All of B's costs on every host; conversation capture is a privacy liability the owner already disabled |

Peers sit between A and B: Claude Code's sandbox inspects no TLS unless a setting turns it on,
Codex intercepts only in its limited mode with the CA key in memory, and Docker Sandboxes
terminate TLS for the hosts where they inject credentials
([sources](546/measurements.md#prior-art-and-platform-sources)).

Decided (D1): B is the target, after A ships.

### M7. Where the proxy runs

| Option | For | Against |
| --- | --- | --- |
| A. In each session's launcher, outside the agent's namespaces, itself confined by Landlock and seccomp | Lifetime is the session's; survives daemon restarts; identity is the listener, so no secrets or ports; a crash affects one session | One small proxy per session (connection pools are not shared) |
| B. Inside the daemon | Direct access to grants and the change feed | A daemon restart cuts every agent's open streams; untrusted traffic parsed in the control process (report 03) |
| C. One shared service (today) | One process | A second lifecycle (#495), fixed ports, a client-identity scheme, one profile per host |

Decided (D2): A.

### M8. Resource limits

The daemon already starts commands in transient systemd user scopes (#29, decided
2026-10-31). The launcher can place each session in its own scope with `MemoryMax`, `TasksMax` and
`CPUWeight`, so a runaway build or fork bomb stops at its session. The user manager is delegated `pids`,
`memory` and `cpu` by default, which covers all three; I/O limits would need a drop-in installed
as root. Where no user manager answers, the launcher falls back to `RLIMIT_NPROC` and
reports the weaker containment.

### M9. WSL2 and macOS

WSL2 runs the Linux design unchanged; the checks that stay specific are the Windows drives,
`/run/WSL` and `WSL_INTEROP`, and the Landlock ABI of Microsoft's kernel (7 in WSL 3.0.3, so no
`RESOLVE_UNIX`). macOS, deferred under #568, would use Seatbelt through `sandbox-exec` with
outbound network limited to the session's proxy port, the same per-session proxy and no CA
under M6 A. `sandbox-exec` is deprecated with no replacement for command-line programs, yet
Codex and Claude Code still ship it. A transparent proxy through a Network Extension needs a
notarized app hosting a system extension, and Endpoint Security an Apple-granted entitlement
and root, so neither fits a per-user command.

## Torlek-native design

```text
torlek daemon ── policy, grants, approvals, evidence, change feed (TUI, phone)
   │  launch spec on an inherited fd          ▲ decisions, requests, counters
   ▼                                          │ (supervisor channel, never in the sandbox)
tmux pane ─> torlek-sandbox (host network; Landlock: its sockets only; seccomp; scope)
               │ serves 127.0.0.1:3128 of the agent's namespace (fd passed back)
               │ resolves, decides, connects out
               ▼
             stage 2 in new user, mount, PID, network, IPC, UTS namespaces:
             mount view, Landlock (files, scopes), seccomp, exec provider
```

1. **Launch.** `cockpit-sessions` asks `cockpit-sandbox` for the profile as today. The native
   backend serializes it with the session's allowlist revision and starts `torlek-sandbox` in
   the pane, with the spec on an inherited descriptor and a supervisor token that never enters
   the sandbox. The launcher brings up the namespaces, receives the listener created inside,
   and only then lets stage 2 `exec` the provider. Any rule it cannot enforce refuses the
   launch with the rule and reason (`Refusal`), as today.
   The mount view binds Codex's socket folder at its fixed path (#446) without an engine
   feature, and a shared desktop session keeps `xdg-dbus-proxy` limited to
   `org.freedesktop.Notifications` (#435).
2. **Traffic.** Each `CONNECT` is checked against the allowlist snapshot. Allowed: connect,
   count. Unknown: answer `403` at once with a body naming the host, the session and the
   request filed, and send the request to the daemon (one per host per session until decided).
3. **Owner decision (#674).** The request appears under Waiting on you, in
   `torlek approval list` and on the phone (#245): allow once (this session), allow always
   for this project (D4), deny. The decision records who, when and through which client, as approvals do (#672).
   The daemon pushes the new revision to the session's launcher; the agent's retry succeeds.
   Revoking a grant closes that host's open tunnels. An agent can ask ahead with a verb such as
   `torlek ask host HOST[:PORT] --reason TEXT`, under its `run-commands` grant.
4. **Policy sources.** Provider hosts and paths come from `cockpit-claude`, `cockpit-codex` and
   `cockpit-opencode` (seeded from Greywall's reviewed profiles, with Apache-2.0 attribution).
   Owner decisions are grants with terms (host pattern, port), like `start-sessions` terms: one
   revision, one audit, one revocation path. Project files can declare hosts they need; like
   `[needs]`, they request and never grant.
5. **Evidence.** Stage 2 reports what it applied: namespaces, Landlock ABI and handled rights,
   the seccomp filter's digest, the network mode. That is the "effective" line, from the
   enforcer rather than parsed from `--help`. The launcher reports bounded counters per host
   (allowed, refused, bytes): automatic observations beside the owner probes of #433.
6. **Readiness.** `torlek doctor` checks user namespaces, the Landlock floor, bubblewrap under
   D3 A and the user manager, by running a short probe launch, not by reading versions.

### What this does better than the engines

- **One lifecycle:** no install step, release pins, service unit, fixed ports, API token,
  client secrets or heartbeat; several Torlek profiles can run on one host.
- **Identity by construction:** a connection's session is the listener it arrived on. The
  client-secret race fixed in #61 and Greyproxy PR #8 cannot happen.
- **Actionable refusals:** under a second, in Torlek's own approval queue, instead of a 30 s
  hold in a dashboard nobody watches (#641).
- **Per-session policy from grants**, revocable mid-session, including open tunnels.
- **Fewer moving parts per session:** one launcher instead of Greywall, up to six helpers and a
  shared proxy; no GPL binary to redistribute; no Go toolchain in the release.
- **Weaknesses 1 to 7 closed by construction** and kept closed by regression tests.

## Migration plan without a gap in enforcement

The profile, refusals and evidence are backend-neutral already, so the switch is per launch.

| Stage | Change | Exit check |
| --- | --- | --- |
| 0 | Theo's decisions recorded; issues filed | This report updated |
| 1 | Native launcher and proxy behind `agents.sandbox_backend = "greywall" \| "torlek"` (owner-only, default `greywall`). A running session keeps the backend it started with; evidence names it. A failed native launch refuses; it never falls back to Greywall | Fixture tests through the daemon; the #433 probe suite passes on both backends in disposable CI |
| 2 | #674 owner decisions and provider-owned allowlists | Unknown host refused under 1 s; decision round trip through the TUI |
| 3 | Red-team (#685 section 1) on the native backend; findings fixed | Report with no open high finding |
| 4 | Default becomes `torlek`; Greywall stays selectable for one release as the rollback | Live Linux and WSL2 runs on the owner's hosts with the sandbox on |
| 5 | Delete `greywall.rs`, `install.rs`, `proxy/`, `seccomp.rs`, pins, `sandbox install`/`restart`/`stop` and their tests; `torlek doctor` offers to stop and remove the old `torlek-greyproxy-<profile>` unit and its state, with the owner's confirmation | No reference to the engines outside history; report 11 and decision register item 15 rewritten |

Selective interception (proposed issue 4) follows stage 1 and gates no stage: nothing Torlek
launches today depends on Greyproxy's interception. Rollback before stage 5 is one owner setting. Nothing in stages 1 to 4 removes the managed
Greyproxy, so an owner who switches back finds it running.

## Decisions for the owner

**D1. Traffic control model.**

- **A (recommended).** Explicit proxy from an empty network namespace, no TLS interception
  (M4 A, M6 A). Smallest, fails closed, no CA.
- **B.** Transparent capture with a TUN device and a Torlek-owned TCP stack (M4 B). Everything
  works without variables; more code, DNS interception.
- **C.** A, plus selective interception with an in-memory per-session CA for hosts that carry a
  bound credential (M6 B), from the start.

**Decided 2026-11-05: C, staged.** The target is C. A ships first; selective interception
with the in-memory per-session CA is the next issue (proposed issue 4).

**D2. Where the proxy runs.** **A (recommended):** in each session's launcher (M7 A): traffic
survives daemon restarts and a session's identity needs no secret. **B:** inside the daemon.
**C:** one shared service, as today.

**Decided 2026-11-05: A.** The egress proxy runs in each session's launcher.

**D3. Namespace and mount setup.** **A (recommended for the first release):** bubblewrap as a
required host program, Landlock and seccomp applied by Torlek (M1 A); revisit B only if
bubblewrap blocks a measured need. **B:** Torlek's own setup code. **C:** Landlock and seccomp
without namespaces.

**Decided 2026-11-05: A.** bubblewrap is a required host program; Torlek applies Landlock and
seccomp itself.

**D4. Scope of "allow always".** **A (recommended):** the project; another project asks again,
so one project's need never widens another's network. **B:** the whole Torlek profile, as the
text of #674 says. Either way stored as grants and revocable.

**Decided 2026-11-05: A.** "Allow always" covers that project only; another project asks
again. It is stored as a revocable grant, and the text of #674 changes from the whole profile
to the project.

**D5. New dependencies.** **A (recommended):** the `landlock` crate (maintained by the Landlock
authors) and `seccompiler` (rust-vmm; last released 2025-03, now developed in the rust-vmm
monorepo), pinned exactly; the workspace keeps
`unsafe_code = "deny"` and their unsafe code is reviewed upstream.
**B:** raw syscalls in one module allowed to use `unsafe`. **C:** `libseccomp` through its C
library.

**Decided 2026-11-05: A.** The `landlock` crate and `seccompiler`, pinned exactly; the
workspace keeps `unsafe_code = "deny"`. The implementing issue adds them; this research adds
no dependency.

**D6. Milestones.** **A (recommended):** stages 1 to 4 in v0.1.0-alpha.4 (sandbox and security),
stage 5 in v0.1.0-alpha.5, macOS in Roadmap: macOS. **B:** a new milestone between alpha.4 and
alpha.5 for the native sandbox. **C:** after the Beta 2 cutover.

**Decided 2026-11-05: A.** Stages 1 to 4 in v0.1.0-alpha.4, stage 5 (remove Greywall and
Greyproxy) in v0.1.0-alpha.5, macOS in Roadmap: macOS.

## Proposed issues (not filed)

Ordered; each is one pull request or a small series. Times are for one agent with focused
tests and do not include review or CI waits.

| # | Title | Acceptance | Milestone (D6) | Supersedes or changes |
| --- | --- | --- | --- | --- |
| 1 | Native Linux launcher behind `agents.sandbox_backend` | Through the daemon with a fake provider: checkout writable; `.git/hooks`, `.git/config`, `.torlek` not; `~/.ssh/id_*` and the state database unreadable; no interface but `lo`; `Seccomp: 2`; a second Unix user starts a session (#628); each provider launched with its own sandbox off; evidence names the backend and Landlock ABI | alpha.4 | #628, #434 |
| 2 | Provider hosts and paths in the provider adapters | Each adapter declares its hosts and folders; a test compares them with the Greywall profiles they replace; attribution kept | alpha.4 | Greywall `--profile` |
| 3 | Per-session egress proxy | Allowed host connects; unknown host `403` under 1 s with a reason; loopback, private, link-local, metadata and IP literals refused; SNI mismatch refused; no DNS or UDP from inside | alpha.4 | #495 |
| 4 | Selective TLS interception for credential hosts | A host with a bound credential (for example the project's `GH_TOKEN`, #754) is intercepted with a per-session CA whose key never leaves the launcher's memory; only the certificate enters the session, with trust variables per runtime; the credential is added on that host only and never enters the sandbox, so `git push` over HTTPS and `gh` work with no token inside; every other host stays a plain tunnel; a pinned client's failure names the host | alpha.4 | #59 placeholders, #754's pass-through, #527 |
| 5 | Network requests as owner decisions | Request listed in `approval list`, Waiting on you and the change feed; allow once, always for the project (D4), deny; who, when, client recorded; revocation closes open tunnels; no path adds an allowlist entry without an owner decision | alpha.4 | #674 (scope text changes per D4), #641 |
| 6 | Live enforcement CI for the native backend | #433's probes on disposable Linux and WSL2 runners, both backends, four sequential launches | alpha.4 | #433 |
| 7 | Red-team the native sandbox | #685 section 1 against the native backend; one issue per finding | alpha.4 | #685 |
| 8 | Default to the native backend | Default `torlek`; doctor's probe launch; rollback documented | alpha.4 | |
| 9 | Remove Greywall and Greyproxy | Stage 5 above, including the confirmed removal of the old unit and state | alpha.5 | #495, #527, #66 wording |
| 10 | Per-session resource limits | A fork bomb or memory hog in one session leaves the others running; fallback reported | Hardening: daemon, sessions, CLI and TUI | |
| 11 | macOS Seatbelt backend | Same probe suite on a macOS runner | Roadmap: macOS | #527, #568 |

### Current alpha.4 issues these supersede

The open issues in v0.1.0-alpha.4 on 2026-11-05, against the proposed issues above. This
report closes and rewrites nothing; Theo does that when filing.

| Current issue | Disposition | By proposed issue |
| --- | --- | --- |
| [#628](https://github.com/Androlax2/agent-cockpit/issues/628) shared seccomp folder | Superseded: the filter is installed from memory; its second-user acceptance becomes a regression test | 1 |
| [#434](https://github.com/Androlax2/agent-cockpit/issues/434) settings files of failed and ended sessions | Superseded: the policy passes on an inherited descriptor, no settings file exists | 1 |
| [#495](https://github.com/Androlax2/agent-cockpit/issues/495) own the Greyproxy lifecycle | Superseded: no shared proxy service; its fresh-home and "present versus working" checks move to the launcher's probe launch | 1, 3, 9 |
| [#641](https://github.com/Androlax2/agent-cockpit/issues/641) request to a new host hangs | Superseded: an unknown host is refused at once and becomes an owner decision | 3, 5 |
| [#674](https://github.com/Androlax2/agent-cockpit/issues/674) refuse at once, owner decides | Kept and rewritten for the native proxy: it can carry proposed issue 5 itself; "allow always" covers the project (D4) | 5 |
| [#433](https://github.com/Androlax2/agent-cockpit/issues/433) live enforcement CI | Not superseded: the same probes stay the Greywall baseline; proposed issue 6 runs them on the native backend, or #433 is widened to both | 6 |
| [#685](https://github.com/Androlax2/agent-cockpit/issues/685) red-team | Not superseded: its section 1 targets the native sandbox once it ships | 7 |
| [#482](https://github.com/Androlax2/agent-cockpit/issues/482) mount source cleanup | Unaffected: the folder stays daemon-owned | |
| [#382](https://github.com/Androlax2/agent-cockpit/issues/382) repository protection | Unaffected | |
| [#546](https://github.com/Androlax2/agent-cockpit/issues/546) this research | Stays open for Theo | |

Outside alpha.4, proposed issue 4 supersedes [#527](https://github.com/Androlax2/agent-cockpit/issues/527)
(Roadmap: macOS) and takes over the proxy placeholders of
[#59](https://github.com/Androlax2/agent-cockpit/issues/59) (v0.1.0-beta.2).

## Sources, limits and what was not measured

- Engines: the pinned trees above, read at those commits; weaknesses 1 to 4 and 6 spot-checked
  at the cited lines. Licenses and provenance: the [earlier report](2026-11-02-546-engine-repositories.md#licenses-and-distribution-inputs).
- Torlek: `crates/` at this branch's base, the decision records for
  [#61](../decisions/2026-11-01-61-sandbox-backend.md), [#495](../decisions/2026-11-02-495-managed-greyproxy.md)
  and [#433](../decisions/2026-11-02-433-sandbox-observations.md), design reports
  [11](../design/greenfield/11-security-isolation-platforms.md) and [17](../design/greenfield/17-agent-channel-and-approvals.md),
  and the issues listed above with their comments.
- Prior art and platform facts: [appendix sources](546/measurements.md#prior-art-and-platform-sources).
- Not measured: Greywall's launch and TUN path; a Rust relay (the Python one is an upper bound);
  WSL2 and Ubuntu with its user-namespace restriction; macOS. The native prototype is a
  measurement aid, not a design: it handles no mount view, no PID namespace and no proxy logic.
