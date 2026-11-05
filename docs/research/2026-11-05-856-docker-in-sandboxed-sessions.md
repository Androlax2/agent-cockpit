# How a sandboxed session may use Docker

Decision issue [#856](https://github.com/Androlax2/agent-cockpit/issues/856), split from #390 by
Theo on 2026-11-05. It builds on the
[native sandbox plan](2026-11-05-546-own-sandbox-and-traffic-control.md) and its decisions
(cited below as "#546 D1" to "#546 D6"). **Research and proposal; Theo's answers are recorded
under [Decisions for the owner](#decisions-for-the-owner) as he gives them.** Merging this
document accepts research delivery only: it authorizes no product code, dependency, host
setting or issue edit. The [issue texts](#changes-to-the-linked-issues-proposed-text) at the
end are proposals; Theo applies them.

## The question

A session in the native sandbox (#829) has an empty network namespace and one proxy listener
(#831). Docker breaks both halves of that: its socket is a way out of the mount view, and its
containers talk to the network from somewhere else. What may a sandboxed session do with
containers, on which engine, with which network, and what does Torlek say about it?

## Recommendation and decisions

Give a sandboxed session a socket to **one engine Torlek runs for its project**, with the
engine's full API, and put the limits on the engine instead of on the requests:

1. The engine is rootless Docker under a Unix user of its own, never the owner's engine.
2. It sees the project's checkouts only, through the same read-only masks a session gets.
3. Its only way to the network is a Torlek egress proxy with the project's grants.
4. No credential and no CA enters a build, an image or a container.
5. Until that engine exists (#390, beta.2), a sandboxed session has no Docker: the mount view
   shows no engine socket, and readiness says so.

- [ ] Q1. What the session holds.
- [ ] Q2. What bounds the engine.
- [ ] Q3. One engine per project or per checkout.
- [ ] Q4. Container egress.
- [ ] Q5. Credentials and the CA in containers.
- [ ] Q6. What alpha.4 ships, and when #390 lands.

## What exists today

Code at this branch's base (`88612ce1`); paths are relative to the repository root.

| Where | Fact |
| --- | --- |
| Native sandbox (#546, M3) | seccomp denies `unshare`, `setns`, namespace flags of `clone` and the new mount API. No engine, Podman or Buildah can start **inside** a session: the engine is always outside, and the only question is which socket the mount view shows |
| `crates/cockpit-daemon/src/serve.rs:29` | An agent inherits eight variables and its XDG layout from the daemon; `DOCKER_HOST` is not among them. The Docker CLI falls back to its default socket |
| `crates/cockpit-sandbox/src/profile.rs:22` | `~/.docker/config.json` (registry logins) is unreadable in every profile |
| `crates/cockpit-readiness/src/project.rs:252` | A rootless engine is `available`, a rootful one is `missing-prerequisite` ("whoever reaches its socket is root on this host"). The probe is `docker info` in the **daemon's** environment (`crates/cockpit-daemon/src/readiness.rs:395`), not the engine a session reaches |
| `crates/cockpit-config/src/lib.rs:177`, `:229` | A project may declare `[execution] environment = "docker"`. `agents.sandbox = "off"` is the one named opt-out, set by the owner for the whole profile |
| `crates/cockpit-daemon/src/verification.rs:396` | The daemon refuses every check whose environment is not `native` (#352) |
| `crates/cockpit-agent-context/bundle/references/sandbox.md:11` | Agents are told to use only the engine their environment provides, never the owner's, and not to start one |
| Report [11](../design/greenfield/11-security-isolation-platforms.md) | "Rootless Docker per project: dedicated engine identity"; "Same-UID separation is not a hostile-tenant boundary"; rejected: "Shared global Docker socket" |

The legacy cockpit (kept until the cutover, #66) already runs the recommended shape on Theo's
host, by hand:

| Legacy piece | What it does | Limit |
| --- | --- | --- |
| `system/etc/systemd/system/agent-docker@.service:7`, `:21`, `:24` | Rootless `dockerd` per project as user `agent-<project>`, in a system unit whose `/home` is empty except the project and its parallel clones | Root setup per project: `useradd`, subordinate IDs, recursive ACLs, three units (`tools/agent-docker.sh:96`). Paths and owner are hard-coded (inventory P85) |
| `agent-docker-relay@.service:11` | `socat` puts the engine's socket in the project folder, which the sandbox already mounts | No API filter: the session holds the engine's full API |
| `tools/agent-docker.sh:3`, `:40` | An nftables table confines the engine users' traffic to Greyproxy; setup refuses without it | The table's rules are not in this repository |
| `agent-docker@.service:13` | `DISABLE_HOST_LOOPBACK=false`: containers reach the host's loopback at `10.0.2.2`, where the proxy listens | Every other loopback service of the host is reachable too |
| `config/bin/.local/bin/greywall-agents/launcher-common.sh:389`, `:402` | Sets `DOCKER_HOST` and a `DOCKER_CONFIG` whose `proxies` inject the proxy into builds and containers | Cooperative variables; the nftables table is the enforcement |
| `config/bin/.local/bin/agent-docker-build`, `docs/agent-docker-build.md:47` | Rewrites a Dockerfile so each `RUN` gets the proxy's CA as a BuildKit secret, never as a layer | Needed only because Greyproxy intercepted every TLS connection. "Scoped trust, not containment of malicious build instructions" |

## Threats Docker adds

The adversary is #546's: the agent and the code it runs, as the owner's user; a kernel exploit
is out of scope, an escape that needs none is not.

| # | Threat | Why it needs no kernel bug |
| --- | --- | --- |
| T1 | **The engine is the owner.** A socket to an engine that runs as the owner (rootless) or as root (rootful) | `docker run -v /:/host` mounts whatever that user reads: `~/.ssh`, Torlek's state, the owner socket (#729). Docker's own guide: "only trusted users should be allowed to control your Docker daemon" |
| T2 | **Masks bypassed.** A container bind-mounts the checkout | It writes `.git/hooks`, `.git/config`, `.torlek`, `.envrc` and reads `.env` (`profile.rs:17`, `:36`): paths the session cannot touch, which later run with the owner's rights |
| T3 | **Egress bypassed.** Pulls, builds and containers use the engine's network | RootlessKit's slirp4netns or pasta is a full NAT with no allowlist: no host rule, no owner request (#674), open DNS and UDP, the LAN, cloud metadata |
| T4 | **Credentials leak into images.** A token or CA passed as build argument, `ENV` or `COPY` | It stays in a layer or a process. #832 exists so a token never enters the sandbox |
| T5 | **Other sessions.** One engine serves every clone of a project | A session mounts a sibling clone's checkout and runs `docker exec` in its containers |
| T6 | **Resources.** Containers run outside the session's scope (#834) | Rootless Docker ignores `--cpus` and `--memory` without cgroup v2 and systemd delegation (report 10) |

## Options

Costs are rough guesses for one agent with focused tests, as in #546.

### Q1. What the session holds

| Option | Threats left | Cost |
| --- | --- | --- |
| A. A socket to a Torlek-run engine of its own project, full API; the limits sit on the engine (Q2 to Q5) | Whatever the engine's bounds leave. A missing bound is a full hole, so each one needs a probe | Part of #390 |
| B. A filtered API proxy in front of an engine: allowed images, no privileged mode, no host mounts | One parser bug from the engine's full rights. `HostConfig` has dozens of escape fields (binds, devices, namespaces, capabilities, volume driver options), BuildKit runs over an upgraded connection, and the API grows each release. Docker's own filter hook was bypassed with an empty body ([CVE-2024-41110](https://www.docker.com/blog/docker-security-advisory-docker-engine-authz-plugin/)). It does nothing for T3 | About 4 days, then upkeep per Docker release |
| C. No socket, ever: containers only through `torlek ask run`, or a Torlek container verb with a declared spec | None from the session. An approved command runs on the owner's engine with the owner's rights, so the approval is the whole control | `ask run` exists. A verb is a week or more and still breaks Compose, Testcontainers and dev containers, which need the API |

A filter is not a boundary on its own; it could be a second layer behind A later.

### Q2. What bounds the engine

Whatever is chosen, the owner's own engine (rootless as the owner, or rootful) is never handed
to a sandboxed session: that is T1, the same as `agents.sandbox = "off"`.

| Option | Threats left | Cost |
| --- | --- | --- |
| A. A dedicated Unix user per project, in a root-installed unit with its own filesystem view (the legacy shape) | A container escape lands as that user inside that view: the project's checkouts and nothing of the owner's. The boundary is the kernel's user separation, set by root where neither agent nor engine can change it | About 3 days for a guided setup and the identity check. The owner runs one `sudo` step per project. Linux and WSL2 with systemd. Files a container creates in the checkout belong to subordinate IDs |
| B. The owner's own user, confined by Torlek's launcher: a mount view without the owner's home, an empty network namespace | The engine cannot run under Landlock ("cannot modify filesystem topology, whether via mount(2) or pivot_root(2)", [kernel](https://docs.kernel.org/userspace-api/landlock.html)) or under #546's seccomp filter, so namespaces are the only wall, and report 11 does not count same-UID separation as a boundary. **Unproven:** bubblewrap maps one user ID, rootless Docker needs 65,536 | A 1-day prototype first, then about 3 days. No root step |
| C. A microVM per project (what Docker Sandboxes does) | A kernel bug inside the guest stays in the guest | Two weeks or more: a guest image to ship and update, KVM on the host, nested virtualization on WSL2 |

### Q3. One engine per project or per checkout

| Option | Threats left | Cost |
| --- | --- | --- |
| A. Per project, shared by its parallel clones (#390's wording, the legacy shape) | T5: a session reaches sibling clones' files and containers. The masks (T2) still hold in every clone | One image store per project |
| B. Per checkout | None between sessions | Every clone pulls and builds its own images (gigabytes, minutes). Under Q2 A, one root step per clone or a pool of users made ahead |

### Q4. Container egress

The session's own proxy cannot serve this: the engine outlives sessions and is shared, and a
proxy's identity is its listener (#546 D2). So the engine gets a proxy instance of its own.

| Option | Threats left | Cost |
| --- | --- | --- |
| A. The engine's network has no route out except a Torlek proxy (the #831 code, one instance per engine), with the project's "allow always" grants (#546 D4) but not the provider's hosts. An unknown host is refused at once and filed as an owner request (#674) naming the project's containers. Registries are hosts like any other | Code in a third-party image reaches the hosts granted to the project: no more than the session itself reaches. Programs that ignore proxy variables fail closed | About 2 days on top of #831 |
| B. The same proxy with a separate allowlist for containers | Less: an image cannot use a host granted for the agent's own work | About 1 day more; the owner answers twice for a host both need |
| C. No network for containers; images arrive only by an owner-approved pull | None | Half a day. Most Dockerfiles download something and fail |
| D. Proxy variables only, the engine's NAT left open | T3 whole | Nothing. Not an option |

How the engine's namespace gets its single listener is #390's design: the file-descriptor
passing of the launcher (#829), or the legacy's nftables rules by user. The acceptance is the
probe.

### Q5. Credentials and the CA in containers

With #546 D1 the proxy intercepts only hosts that carry a bound credential, and the CA's key
lives in one session launcher's memory (#832). An engine proxy has neither.

| Option | Threats left | Cost |
| --- | --- | --- |
| A. None. The engine's proxy never intercepts; from a container a credential host is a plain tunnel with no token added. No CA exists, so `agent-docker-build` is not carried over | None added. A build that needs a private dependency or image asks the owner | Nothing |
| B. Interception for the engine's own pulls only, when a project needs a private registry: the daemon trusts a per-engine CA, containers and builds get nothing | The registry credential serves any pull the agent asks for on that host | About 2 days, when a project needs it |
| C. The CA and credentials reach builds and containers as secret mounts, never as `ARG`, `ENV` or `COPY` | Any image the agent runs acts with the project's credentials; a Dockerfile can copy a mounted file | 3 days or more, and a Dockerfile rewriter to maintain |

### Q6. What alpha.4 ships, and when #390 lands

`agents.sandbox` is profile-wide. Once #833 turns the sandbox on, a project that needs Docker
inside its sessions has two ways until #390 exists: `torlek ask run`, or `off` for every
project.

| Option | Threats left | Cost |
| --- | --- | --- |
| A. Alpha.4 ships "no Docker in a sandboxed session": #829 proves no engine socket is reachable, readiness says `unsupported`. #390 stays in beta.2 | None new | Docker projects (Stadeo's Compose) lose in-session Docker from #833 to beta.2 |
| B. The same alpha.4 gate, and #390 moves into alpha.4 after #833 | None new at #833; #390's surface arrives sooner, with the red-team (#685) right behind it | About a week of #390 inside alpha.4 |
| C. #390 gates #833 | The unsandboxed period (#729) lasts a week longer | The default flips later |

### What `torlek doctor` and readiness report

This follows from the answers; with the recommended ones, for a project that declares Docker:

| State | `docker` requirement |
| --- | --- |
| `agents.sandbox = "off"` | Today's rule: rootless `available`, rootful `missing-prerequisite`; the detail adds that agents reach it unsandboxed |
| `required`, before #390 | `unsupported`: "containers are not available to sandboxed sessions in this release"; next: `torlek ask run` |
| `required`, no Torlek-run engine for the project | `missing-prerequisite`, with the setup step |
| `required`, the engine answers but is not the recorded one, not rootless, or runs as the owner | `missing-prerequisite`: the identity that failed, by name |
| `required`, verified engine | `available`: engine ID, its user, "egress through Torlek's proxy" |

The probe asks the engine a session would reach, not the daemon's `docker` (today's mismatch).

## Decisions for the owner

**Q1. What the session holds.** **A (recommended):** a socket to a Torlek-run engine of its own
project, full API, limits on the engine. **B:** a filtered API proxy. **C:** no socket, ever.
Most secure: C.

*Pending.*

**Q2. What bounds the engine.** **A (recommended):** a dedicated Unix user per project, one
`sudo` step per project. **B:** the owner's user confined by Torlek's launcher, after a
prototype. **C:** a microVM per project. Most secure: C, then A.

*Pending.*

**Q3. Engine scope.** **A (recommended):** one per project, shared by its clones. **B:** one
per checkout. Most secure: B.

*Pending.*

**Q4. Container egress.** **A (recommended):** a Torlek proxy on the engine with the project's
grants. **B:** the same with a separate container allowlist. **C:** no network for containers.
Most secure: C, then B.

*Pending.*

**Q5. Credentials and CA.** **A (recommended):** none in builds, images or containers.
**B:** interception for the engine's own pulls, when a private registry needs it. **C:** secret
mounts into builds and containers. Most secure: A.

*Pending.*

**Q6. Alpha.4 and #390.** **A (recommended):** alpha.4 ships no Docker in sandboxed sessions,
and #390 stays in beta.2. **B:** #390 moves into alpha.4, after #833. **C:** #390 gates #833.
Most secure: A.

*Pending.*

## Changes to the linked issues (proposed text)

Written for the recommended answers; revised below each decision once Theo has answered.
Nothing here is applied by this report.

### #390 (covered; rewrite Scope and Acceptance)

> **Scope**
>
> - **Engine.** Rootless Docker per project under a Unix user of its own, in a unit whose
>   filesystem view holds that project's checkouts only. A guided root step sets it up
>   (`torlek doctor` prints it; the owner runs it). Never the owner's engine, never rootful.
> - **Masks.** The engine sees each checkout through the session baseline's denials
>   (`.git/hooks`, `.git/config`, `.torlek`, `.envrc` read-only; `.env` unreadable).
> - **Session.** The launcher (#829) binds the engine's socket into the session's mount view
>   and sets `DOCKER_HOST`. Full API, no request filter. The engine's identity (recorded
>   engine ID, rootless, its user) is verified before the socket is bound.
> - **Egress.** The engine's network has no route out except a Torlek egress proxy (#831's
>   code, one instance per engine) with the project's "allow always" grants. Unknown hosts are
>   refused at once and filed as owner requests (#674). The session's `DOCKER_CONFIG` injects
>   the proxy into builds and containers.
> - **Credentials.** None in builds, images or containers, and no CA. `agent-docker-build` is
>   not carried over.
> - **Forwards** to a container's published port use the mechanism #860 decides.
> - **Limits.** The engine runs in a scope of its own; readiness names what is enforced (#834).
>
> **Acceptance** (through the daemon, on Linux and WSL2)
>
> - From a sandboxed session `docker run` works; a container that mounts `/` sees no file of
>   the owner's home, Torlek's state or another project.
> - A container cannot write `.git/hooks`, `.git/config`, `.torlek` or `.envrc` of any
>   checkout, nor read `.env`.
> - From a container, direct TCP, DNS and UDP fail; a granted host connects through the proxy;
>   an unknown host is refused in under 1 s and listed as a request naming the project.
> - A session of another project cannot reach this engine's socket.
> - An engine that is rootful, runs as the owner, or is not the recorded one is refused, with
>   the reason; readiness reports each state as in #856's table.
> - No build argument, layer or container environment holds a credential or a CA.
>
> **Depends on:** #829, #831, #674, #856

### #352 (covered; replace the container part of Scope)

> - A check whose environment is `docker` runs in a container of the project's Torlek-run
>   engine (#390), started by the daemon's supervisor: the same engine, masks and egress proxy
>   a session's containers get. Never the owner's engine.
> - Without that engine the receipt is `unavailable`, naming the setup step.
>
> **Depends on:** #313, #390, #862 (the sandbox environment)

### #829 (add to Acceptance)

> - No container engine socket is reachable from the session (`/var/run/docker.sock`,
>   `$XDG_RUNTIME_DIR/docker.sock`, Podman's and containerd's), and `DOCKER_HOST`,
>   `DOCKER_CONTEXT` and `DOCKER_CONFIG` are unset: `docker info` fails with "no engine".
> - The launch spec can name a Unix socket to show at a path inside the view, allowed by
>   Landlock where the kernel scopes named sockets; unused until #390, so #390 changes no
>   launch code.

### #831 (add a note)

> The proxy's owner is a listener, not always a session: #390 runs one instance for a
> project's container engine. Keep the decision core free of session-only assumptions: its
> input is an allowlist snapshot and a subject (a session, or a project's containers), and
> requests and counters carry that subject.

### #832 (add a note)

> Interception serves session listeners only. A proxy instance in front of a container engine
> (#390) never intercepts and holds no credential: from a container, a host with a bound
> credential is a plain tunnel. The per-session CA certificate is never copied into a build
> argument, an image or a container.

### #674 (add a note)

> A request can come from a project's containers (#390) instead of a session. It names the
> project and "containers"; "allow once" lasts until that engine stops; "allow always" is the
> same project grant a session's request creates (#546 D4). The feed event carries the
> subject, so the phone (#245) can show which one asked.

### #833 (add to Acceptance)

> - With the sandbox on, a project that declares Docker reports `docker` as `unsupported`
>   ("containers are not available to sandboxed sessions in this release", next action
>   `torlek ask run`) until #390 ships; `torlek doctor` shows it before the first launch.

Also touched, one line each: #685 gains three probes once #390 ships (T1 socket escape, T2
mask bypass, T3 egress bypass); #862 can reuse #831's subject for checks and plugins; #834's
limits do not cover an engine, which gets its own scope in #390.

## Sources, limits and what was not checked

- Code and legacy files as cited, read at `88612ce1`. Issues #390, #352, #829, #831, #832,
  #833, #674, #860, #862 and #834 with their comments, on 2026-11-05.
- Docker: [rootless mode](https://docs.docker.com/engine/security/rootless/) (65,536
  subordinate IDs; daemon and containers in a user namespace),
  [security](https://docs.docker.com/engine/security/) (daemon attack surface),
  [rootless tips](https://docs.docker.com/engine/security/rootless/tips/) (limits need
  cgroup v2 and systemd). RootlessKit's network modes: #546's
  [appendix](546/measurements.md#prior-art-and-platform-sources).
- Nothing was run: no engine was started and no option was prototyped. Q2 B's feasibility,
  the masks surviving a recursive bind into a container (T2), and an engine behind a single
  listener (Q4 A) are claims for #390's tests to prove.
- Not covered: macOS (every engine there runs in a VM; Roadmap: macOS), Podman and other
  engines (the same bounds apply to anything that speaks the Docker API), where the image
  store sits among the caches of #859.
