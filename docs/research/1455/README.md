# Does Docker Sandboxes replace Torlek's own sandbox and container crates?

For Theo, deciding whether Torlek keeps `cockpit-sandbox`, `cockpit-sandbox-launcher` and
`cockpit-containers` or builds on [Docker Sandboxes](https://docs.docker.com/ai/sandboxes/).
Issue [#1455](https://github.com/Androlax2/agent-cockpit/issues/1455).
**Research and proposal, 2026-11-24; Theo has not decided.** Merging this document accepts
research delivery only. It changes no code, installs nothing and adds no dependency. The
Docker pages read, with the sentences quoted here, are in the appendix
[sources.md](sources.md).

## Answer

**Keep ours (option A).** Docker Sandboxes isolates harder than Torlek does: one microVM with
its own kernel per sandbox, where Torlek shares the host's kernel. It still does not replace
the three crates, for five reasons:

1. **It supports the platforms Torlek does not, and not the ones Torlek does.** On Linux
   Docker supports Ubuntu 24.04 or later with KVM and no other distribution; users report the
   unsupported archive working elsewhere. Torlek's release platforms are Linux and WSL2, with
   an Arch recipe and a Debian package. macOS and Windows 11, where Docker is strongest, are
   where Torlek has no sandbox at all.
2. **Every Torlek user would need a Docker account and a proprietary binary.** `sbx` refuses
   to run without a sign-in, which Docker's maintainers call "purely a business reason" with
   "no plans to change this". It publishes no source, sends telemetry unless turned off, and
   is at version 0.54 with breaking changes in each of its last three releases (0.51.0, 0.53.0, 0.54.0).
3. **The owner's sandbox decisions do not fit its model.** Its policy is per machine or per
   sandbox, never per project (#546 D4). A directly mounted checkout lets the agent write
   `.git/hooks` and `.git/config`. Its agents may push with an injected token (#857 says the
   daemon pushes). It forwards the owner's SSH agent by default.
4. **A virtual machine cuts the wires Torlek runs on.** The agent socket, the provider
   sockets, the hook bridge and the process tree all assume one kernel. Each needs a new
   transport, and the agent socket's identity check (#546 D2) needs a new design.
5. **It would not remove the code.** A confined `ask run` (#1387) and its root part run on the
   host whatever sandboxes the session. About a third of the three crates stays under
   option C.

What Docker Sandboxes would give: a kernel boundary, root inside the sandbox, Docker without
a root step per project, and a sandbox on macOS and Windows. The last one is the only gain
Torlek has no plan for, so it is the one place to look at Docker Sandboxes again
([Q3](#decisions-for-the-owner)).

The kernel boundary is real but it has not been the weak point. Docker has published
[twelve CVEs](#dockers-own-security-record) for the product in five months, none in the
hypervisor: they are in the host-side services around it, the same kind of code Torlek
maintains today.

- [ ] Q1: replace Torlek's sandbox with Docker Sandboxes? Recommended: A, no.
- [ ] Q2: build a Docker Sandboxes backend beside ours on Linux now? Recommended: A, no.
- [ ] Q3: when macOS is scheduled, prototype Docker Sandboxes as that platform's sandbox?
  Recommended: A, yes, as a two-week prototype.

## Method and labels

- **Docker.** Docker's own pages on 2026-11-24: the Sandboxes and AI governance sections of
  docs.docker.com, the `sbx` command reference, the release notes up to 0.54.0 (2026-11-16),
  the pricing page, the Subscription Service Agreement and the `docker/sbx-releases`
  repository. Nothing was installed or run and no account was created.
- **Public sources.** Docker's blog, the CVE records Docker filed with the National
  Vulnerability Database, the `docker/sbx-releases` issue tracker and one independent
  comparison (Kudelski Security, 2026-11-23).
- **Torlek.** `crates/` at `09335ceb`, the owner decisions on #546, #856, #857, #858, #860,
  #861 and #1158, and the alpha 8 red-team run on
  [#685](https://github.com/Androlax2/agent-cockpit/issues/685).
- **Labels.** *Docker's docs*: what a Docker page says, quoted or closely paraphrased, with
  its page in the appendix. *Reported*: what a user wrote on Docker's tracker or a third
  party published; one person's observation, not Docker's statement. *Inference*: a
  conclusion of this research from those sources. *Not tested*: applies to every statement
  about how Docker Sandboxes behaves, since none was observed. *Est.*: a cost counted from
  interfaces, not from a prototype.
- **Product history.** The product changed shape twice in a year: containers inside Docker
  Desktop's VM (experimental preview, 2025-11-25), then microVMs in Docker Desktop 4.58
  (2026-01-26), then the standalone `sbx` command; Docker Desktop 4.80 (2026-06-29) removed
  the `docker sandbox` plugin. This report describes `sbx` 0.54.0.

## What Docker Sandboxes does

All rows are *Docker's docs* unless marked.

| Question | Answer |
| --- | --- |
| Isolation | One microVM per sandbox with its own Linux kernel. "The primary trust boundary is the microVM. The agent has full control inside the VM, including sudo access." KVM on Linux, Windows Hypervisor Platform on Windows, Hypervisor.framework on macOS (Docker's blog). Docker built the virtual machine monitor itself ("we built a new VMM") and publishes no source or third-party audit |
| Filesystem | The workspace is mounted read-write through virtiofs at the same absolute path. Nothing else of the host is visible. Extra folders or single files can be mounted, each read-only if asked. `--clone` mounts the repository read-only and gives the agent a private clone, which the host fetches from. A host Git worktree mounted directly has no Git access inside |
| What the mount does not protect | "There is no boundary between the agent's edits and your host filesystem." The agent can write `.git/hooks`. Clone mode protects the repository "from modification, not from inspection": `.env` files stay readable. No documented way to hide a path inside a mounted workspace |
| Network egress | "All outbound TCP traffic from the sandbox routes through a proxy on your host": a forward proxy for HTTP and HTTPS, a transparent path for other TCP. UDP is off by default, ICMP always. Three machine-wide presets: Open (allow all), Balanced (deny by default with a broad baseline that includes `*.googleapis.com`, package managers, code hosts and cloud services), Locked Down (deny all) |
| Allow-lists | Rules name hosts, wildcards, addresses or CIDR ranges, with ports, and optionally HTTP methods and paths. A rule is global or scoped to one sandbox. A deny wins over an allow |
| Egress decided at run time | Under Balanced or Locked Down an unmatched request files an approval. "The request that triggered the approval doesn't wait for your answer. It's denied." The owner answers in the terminal dashboard or with `sbx policy approval respond`. An approval covers one destination and port for the sandbox that asked |
| Egress weakness *reported* | Issue 261, open: a built-in agent's own allowed hosts stay reachable under a deny-all default; a maintainer calls it "either a bug, or at the very least, currently undefined behavior" |
| Egress weakness Docker states | A denied host that shares an address with an allowed host is reachable in three cases: on connections the proxy does not decrypt, on later cleartext requests of one connection, and on a connection that names no host |
| Credentials | Kept on the host in the OS keychain (a file on headless Linux). The forward proxy "overwrites the auth header"; the sandbox holds a sentinel. Built in for Anthropic, OpenAI, GitHub, Google and seven more; custom ones are experimental. Not protected: `-e` variables, a kit with `passthrough: true`, Kiro's login |
| TLS | On the `forward` path the proxy terminates TLS with its own certificate, which the sandbox trusts; other paths are not decrypted. *Reported* (tracker issue 176): only hosts with an injected credential are decrypted, by a CA named "Docker Sandboxes Proxy CA". Containers the sandbox starts do not inherit that trust |
| Host-side reach | Supported agents connect to an MCP gateway on the host, and "local stdio MCP servers run on the host, not inside the sandbox VM" |
| SSH | "SSH agent forwarding is enabled by default. Private keys stay on the host, but any process inside the sandbox can ask the forwarded agent to authenticate or sign data." *Reported* (issue 121, open): it authenticated to a code host with no secret configured. Turned off by a machine-wide setting |
| Docker in the sandbox | Each sandbox runs its own Docker Engine. "The agent has no path to your host Docker daemon." Images are shared neither with the host nor between sandboxes. The Docker disk is 10 GB per sandbox by default |
| Agents | claude, codex, copilot, cursor, devin, docker-agent, droid, kiro, opencode and a plain shell. Gemini was retired in 0.53.0; there is no Qwen. Each starts with its permission prompts off (`claude --dangerously-skip-permissions`, `codex --dangerously-bypass-approvals-and-sandbox`): "The sandbox itself is the safety boundary." Another agent is packaged as a kit, an experimental feature |
| Platforms | macOS 14 or later on Apple silicon; Windows 11 on x86-64; Ubuntu 24.04 or later on x86-64 or Arm with KVM. "Docker does not test or support Docker Sandboxes on Ubuntu derivatives." A Rocky Linux 8 package and a generic Linux archive are published without a support statement. *Reported* (issue 106): the archive runs on Arch and Debian 13. Docker's FAQ and release notes mention WSL2 setups without stating support. Inside another VM it needs nested virtualization (Docker's blog) |
| Docker Desktop | Not needed: "You don't need Docker Desktop or Docker Engine to use `sbx`." It is the `sbx` command and a `sandboxd` daemon |
| Licence and price | Proprietary: "Copyright © 2026 Docker Inc. All rights reserved." No source. Local use is free, "including for commercial and professional work". Organization policy, sign-in enforcement and audit logs belong to the paid AI Governance plan. Cloud sandboxes are billed by the hour |
| Sign-in | Required. "Signing in gives each sandbox a verified identity." CI signs in with a personal access token. On the tracker Docker's maintainers say there are "no technical reasons for the login" and "no plans to change this" (issues 5 and 321) |
| Headless | Yes, after `sbx policy init`; confirmations need `--force` |
| Offline | Not documented. A cached image is used when a pull fails; the sign-in and Docker's hosts are needed. *Reported* (issue 609, open): a signed-in `sbx` stopped working while `login.docker.com` could not be reached. Telemetry is on by default, with an opt-out |
| Interface for another program | The command line, with `--json` on most commands and exit statuses. No documented local API; the API and SDK are for cloud sandboxes. A reply on Docker's tracker (2026-11-18, issue 884) says a supported local API is being worked on. The JSON shape changed in 0.48.0, and 0.51.0, 0.53.0 and 0.54.0 each list breaking changes |
| Records | `sbx policy log`: hosts allowed and blocked with a count, no method or path, no filesystem events. Audit logs need the paid plan |
| Resources | Memory defaults to half the host's, between 512 MiB and 32 GiB, per sandbox; CPUs default to all. A 20 GB root disk per sandbox. "Sandboxes trade higher resource overhead (a VM plus its own daemon) for complete isolation." No start time is published beyond "start in seconds" |
| Speed of the mounted workspace | Docker publishes no figure. *Reported*, all on macOS hosts: a Maven build took 13:12 on the mount against 2:39 on the sandbox's own disk (issue 31, open); `cp -a` of small files was about 53 times slower than in the sandbox's `/tmp` (issue 662, open). One Linux report: an idle sandbox held 1.4 GB of host memory (issue 828). No Linux file benchmark was found |

## Side by side with Torlek

Torlek's column is the code at `09335ceb`: the crate READMEs of
[`cockpit-sandbox`](../../../crates/cockpit-sandbox/README.md),
[`cockpit-sandbox-launcher`](../../../crates/cockpit-sandbox-launcher/README.md) and
[`cockpit-containers`](../../../crates/cockpit-containers/README.md).

| Mechanism | Torlek today | Docker Sandboxes | Fit |
| --- | --- | --- | --- |
| Boundary | bubblewrap namespaces, then Landlock and a seccomp deny list applied by `torlek-sandbox` from memory (#546 D3, D5). The host's kernel is shared; #546's threat model leaves a kernel exploit out of scope | A hypervisor and a kernel per sandbox | Docker is stronger |
| Rights inside | The owner's user, no capability, no new namespace, no mount | Root through sudo | Docker gives more: system packages install without an `ask run` |
| Checkout | Read-write, with `.git/hooks`, `.git/config`, `.torlek`, `.envrc` and editor task files read-only and `.env` files hidden | Read-write whole, or a read-only source with a private clone. Read-only single paths may be expressible as extra mounts (*inference*, *not tested*); hiding `.env` is not | Torlek is stricter |
| Home, caches, tool configuration | A home per session; no owner configuration or cache mounted (#859) | Nothing of the home; the image's own | Equal |
| Egress default | Deny. The allow-list is the provider adapter's hosts, the checkout's registries and the owner's grants, each entry naming its source | Deny under two of three presets, with a baseline wider than any Torlek adapter under Balanced | Torlek is narrower |
| Egress decision | Refused at once with `403` and a reason; filed as a network request; the owner allows once, allows always **for the project** (#546 D4) or denies; a revoked grant closes its open tunnels | Refused at once; filed as an approval; allow for **that sandbox**, or a global rule by hand | Close, except the scope. Torlek would replay a project's grants into each sandbox |
| Names and addresses | No name resolves inside. The proxy resolves, refuses loopback, private, link-local and metadata addresses unless granted, and on 443 checks the TLS server name against the `CONNECT` host | A resolver inside that enforces policy; the three stated shared-address cases; private ranges under Balanced need a rule | Torlek is stricter |
| Identity of a connection | The listener it arrives on: one proxy per session, in that session's launcher (#546 D2). No secret | One host proxy; the sandbox | Equal in effect |
| Credentials | Bound at the proxy; the session holds a placeholder; the real value is added on that credential's own host only (#858 Q1) | Same model, with more providers built in | Equal; Docker has more ready-made |
| TLS interception | Only on hosts with a bound credential, by a CA made in memory per session and name-constrained to those hosts (#546 D1) | On the forward path; *reported* as credential hosts only. No documented name constraint or per-sandbox CA | Close in scope; Torlek's CA is narrower |
| Pushes | The daemon pushes (`torlek push`); the proxy refuses `git-receive-pack` on a bound host; the owner grants the push right per host (#857, #155) | The agent pushes itself with the injected GitHub token or the forwarded SSH agent. An HTTP path rule might block it (*inference*, *not tested*) | Torlek only |
| Host services | Declared services, each granted per project, relayed byte for byte by the launcher (#860, #880) | `host.docker.internal` with an allow rule for `localhost:PORT` | Docker is simpler; no declared service or identity check |
| Docker | A rootless engine per project under a Unix user of its own, shared by the project's clones, seeing checkouts through the session's masks, with egress through a Torlek proxy (#856 Q1 to Q5). One root step per project | An engine per sandbox inside its VM. No root step. No shared image cache | Docker is simpler and removes the #856 threats T1 and T5 by construction; it costs disk and pulls per session |
| Host commands | `ask run`: the owner approves a digest; the run is confined to its card's folders and hosts; named files are pinned (#1387) | None. The owner works on the host | Torlek only; unchanged by any option |
| Programs without an adapter | The floor and no host; every host an owner decision (#861 Q1, Q2) | A kit with its own rules | Torlek is stricter |
| Evidence | Every launch records what enforced each rule, bubblewrap's version and the Landlock ABI; refusals, counters and forwards per session | `sbx inspect` and the per-host policy log | Torlek records more |
| Resource limits | None per session yet (#834) | CPU and memory per sandbox | Docker only |
| Refusal when absent | `required` on a host without the backend refuses every launch; never an unsandboxed fallback | `sbx` refuses without KVM, a sign-in or a preset | Equal |

### The owner decisions, one by one

| Decision | Holds on Docker Sandboxes? |
| --- | --- |
| #546 D1: explicit proxy, selective interception with an in-memory per-session CA | No. Docker's proxy and its CA are Docker's; Torlek cannot choose which hosts are intercepted |
| #546 D2: the proxy runs in each session's launcher | No. One host daemon serves every sandbox |
| #546 D3: bubblewrap required, Landlock and seccomp by Torlek | Replaced by the hypervisor |
| #546 D4: "allow always" covers the project | No. Per sandbox or global; Torlek would keep the grant and replay it |
| #546 D5: `landlock` and `seccompiler` pinned exactly, `unsafe_code = "deny"` | Replaced by a binary with no source to pin or review |
| #856 Q1 to Q5: a project engine, bounded, egress through Torlek's proxy, no credential or CA in a container | Q1 and Q2 are met more strongly by a private engine per sandbox; Q3 is the next row. Q4 changes owner: container traffic follows Docker's policy, not the project's grants. Q5 holds: no credential and no CA reaches a container |
| #856 Q3: one engine per project, shared by its clones | No. One per sandbox, with its own image store |
| #858 Q1: provider credentials bound at the proxy; a self-refreshing login only by a per-project grant | The first half holds. The grant has no equivalent: Docker's OAuth logins are global or per sandbox |
| #858 Q2: an account changes only at a relaunch | Holds if Torlek sets the sandbox's secret before each launch |
| #861 Q1, Q2: an unadapted program gets the floor and no host | Holds under Locked Down with a kit that adds no rule, if the kit's own hosts can be removed (issue 261 reports that a built-in agent's cannot) |
| #861 Q3: an adopted process is shown `UNSANDBOXED, adopted` | Unaffected |
| #1158: red-team checks run inside the real sandbox as an agent session | The harness would need the same for a second backend |

## Against the security bar

The bar is the alpha 8 run on #685: "no sandbox escape, no privilege gain, no forged
authority, no write outside the checkout", against the native backend. Docker's column is
what its documentation says would happen (*not tested*).

| Alpha 8 check | Torlek's result | Docker Sandboxes, from its docs |
| --- | --- | --- |
| 1.1 to 1.3: read another home, `~/.ssh`, write another project | Not mounted | Not mounted either. But the owner's SSH agent is forwarded by default, so a process can authenticate and sign as the owner until a machine-wide setting turns it off |
| 1.4: a host that is not allowed | Blocked by the proxy | Blocked under Balanced and Locked Down. Balanced starts from broad wildcards, a built-in agent's hosts are *reported* to stay allowed under deny-all (issue 261), and Docker states three cases where a denied host is still reached |
| 1.5: `/tmp`, shared paths, other processes, sockets | All blocked; `.git/config` and hooks read-only | A VM has its own `/tmp` and processes and no host socket. `.git/hooks` is **writable** in a direct mount; Docker's advice is to review before trusting, or clone mode. The sandbox can ask the host to open a URL in the owner's browser by default |
| 1.6: seccomp and the launcher | Five of six blocked; `personality` allowed (#1449) | Not comparable. The agent is root in its VM by design; the hypervisor is the control. A kernel bug that breaks Torlek's sandbox does not break this one |
| 2.1 to 2.4: self-grant, self-approve, forged approval | All refused | Not the sandbox's work in either design. Torlek's refusals rest on the agent socket, which a VM cannot reach as it is |
| 4.1, 4.2: an approved run with a hidden write | Stopped by the run's confinement | Unchanged: an approved run executes on the host under #1387 |
| #1118: container escapes (not yet run) | Rests on the engine's user, masks and proxy | A private engine in the VM has no path to the owner's engine or another project's |

By design Docker Sandboxes would pass the escape checks at least as well as Torlek on its
supported platforms, and it would fail two that Torlek passes unless configured: the
checkout's Git hooks and the SSH agent. None of alpha 8's evidence transfers: a backend
change needs its own red-team run.

### Docker's own security record

Docker's CVE authority published twelve CVEs naming Docker Sandboxes between 2026-06-18 and
2026-11-12 (National Vulnerability Database, read 2026-11-24; the scores are Docker's own
CVSS 4.0). None is in the hypervisor. All are in host-side code that serves the sandbox:

| Area | CVEs | What a sandbox could do |
| --- | --- | --- |
| Workspace sharing | CVE-2026-77179 (9.4, macOS), CVE-2026-18171 (5.7) | "escape the shared workspace, and read or modify arbitrary host files"; write to a host folder attached read-only |
| Host-side helpers | CVE-2026-79994 (8.7), CVE-2026-103916 (8.4), CVE-2026-121614 (7.1) | Make the host connect to a Unix socket outside the workspace; make `sbx cp` read or write outside it; run a program on the host through the repository's Git configuration |
| Credential proxy | CVE-2026-101998 (5.9), CVE-2026-105452 (5.9), CVE-2026-105570 (6.7) | Recover the tokens the proxy is meant to keep outside the sandbox |
| Egress | CVE-2026-12039 (5.7), CVE-2026-12539 (5.7) | Send data out through DNS, or through ICMP after a daemon restart |
| Daemon | CVE-2026-126329 (7.3), CVE-2026-126330 (2.4) | Affect a daemon run as root; a removed port published again |

Three things follow for this decision:

- A hypervisor moves the boundary; it does not remove the host-side file server, proxy and
  helpers, and those are where the escapes were. Torlek's equivalents are the launcher's
  proxy and the push route, which the owner can read and the red-team harness exercises.
- The Git configuration and hook findings match Torlek's own protected paths: the paths
  Torlek makes read-only are the ones a writable mount leaves open.
- No independent escape test or audit of Docker Sandboxes was found. The one independent
  comparison scores it 4.5 out of 5 and says "We did not try to escape the sandbox of each of
  these tools" and that being closed source "makes it more difficult to audit".

Torlek's sandbox has no CVE record and one red-team run by its own project, which is weaker
evidence than a public record. The comparison says where each design has failed or been
tested, not which is safer.

## What Torlek would gain and lose

| Gain | Loss |
| --- | --- |
| A kernel per session: a kernel exploit, out of scope today, comes into scope and is covered | A supported sandbox on Arch, Debian, Fedora and Ubuntu before 24.04, and any sandbox on a host without KVM or nested virtualization. WSL2 is not on Docker's supported list |
| A sandbox on macOS (Apple silicon) and Windows 11, where Torlek has none and #546 found only a deprecated route for macOS | A Docker account for every user, a proprietary binary and telemetry to switch off |
| Root inside: system packages and services without an owner prompt | "Allow always" per project, grants that name their source, and revocation that closes open tunnels |
| Docker with no root step per project and no engine masks | Protected Git hooks and configuration and hidden `.env` in the checkout |
| Resource limits per session (#834) | The push route's guarantee that no agent pushes by itself |
| Eleven credential providers ready-made | The harness versions Torlek tests: the agent in a sandbox is the image's. "Updating the `sbx` CLI or pulling an updated template doesn't update agents inside existing sandboxes." |
| Less of our own security code to maintain | Per-launch evidence, and a sandbox whose source the owner can read |
| | Memory and disk per session: a VM, a 20 GB root disk and a 10 GB Docker disk each, where bubblewrap adds about 2 ms and no memory (#546, measured) |
| | Checkout speed, if the macOS reports hold on Linux: builds and copies on the mount several times slower |

## Options

Sizes: the three crates hold 18,396 source lines and 11,471 lines of tests in 527 test
functions (`cockpit-sandbox` 8,143, `cockpit-sandbox-launcher` 8,174, `cockpit-containers`
2,079). Times are for one agent with focused tests and are *est.*: nothing was prototyped.

### A. Keep ours

- **Cost now:** none.
- **Cost that continues:** the three crates; exact pins of `landlock`, `seccompiler`, `rustix`,
  `rustls`, `rcgen` and `httparse`; the open work already filed (#833 default-on, #1118
  container red-team, #1449, #834 limits).
- **What stays unsolved:** macOS (#568) and native Windows have no sandbox, and the only
  native macOS route #546 found is the deprecated `sandbox-exec`.

### B. Docker Sandboxes as an optional backend beside ours

A second variant of `cockpit_sandbox::Backend`, chosen by the owner, on a host with `sbx`
installed, signed in and at a version Torlek has tested.

| Work | Est. |
| --- | --- |
| Probe and refusals: `sbx` version, sign-in, preset, SSH forwarding off, browser opening off | 2 days |
| Launch: the launch spec turned into `sbx create` and `sbx run`, the session's home and instruction bundle mounted, the harness started with Torlek's own flags and hooks instead of Docker's permission-skipping defaults | 1 week for the first harness, 2 to 3 days for each of the other three; Qwen needs a kit |
| The agent socket across a VM: a TCP listener reached through `host.docker.internal`, with a per-session secret in place of "the session is the socket it arrived on". This changes a [protocol rule](../../../.agents/rules/protocol.md) and needs its own decision and red-team. Docker has a relay for Unix sockets inside the workspace, but it is known only from CVE-2026-79994 and is not a documented interface | 1 to 2 weeks |
| Provider sockets the other way (Codex's app server, OpenCode's server) through `sbx port publish` | 3 to 5 days |
| Egress: Torlek's allow-list and project grants written as sandbox-scoped rules at each launch; Docker's approvals polled into Torlek's network requests and answered from Torlek; a revoked grant removed. A built-in agent's own allowed hosts may stay reachable whatever Torlek writes (issue 261), so the harnesses may need kits of Torlek's own | 1 week |
| Credentials set per sandbox before launch; a rule that keeps the push route (deny `git-receive-pack`), proven by a test | 3 days |
| Evidence, `torlek session sandbox`, readiness and `torlek doctor` | 3 days |
| Checks (#352, #862) and Docker work through `sbx exec` and the sandbox's own engine | 3 to 5 days |
| Tests against a fake `sbx`, and a live job that needs KVM and a Docker account token in CI | 1 week |
| A red-team run on the new backend with new probes (SSH agent, browser opening, clipboard, hooks) | 3 days and the owner's time |
| **Total** | **9 to 11 weeks** (43 to 55 working days), then tracking a 0.x command line that shipped three releases in the two weeks before 2026-11-16 |

It removes nothing: both backends are maintained, tested and red-teamed. On Linux it would
serve only Ubuntu 24.04 or later, where ours already works.

### C. Replace ours

Everything in B for all four harnesses, without the fallback, then the removal.

- **Removed:** `cockpit-containers` whole, and the launcher's namespace, Landlock, seccomp,
  session proxy, interception, forwards and engine code: about 11,000 to 12,000 of the 18,396
  source lines (*est.* by module).
- **Kept:** about a third. A confined `ask run`, its card-host proxy and its root part (#1387)
  run on the host, so `root_run`, `root_route`, `run_egress`, the proxy core and the hand-off
  stay.
- **Lost outright:** a supported sandbox on every Torlek installation that is not Ubuntu
  24.04 or later, macOS 14 on Apple silicon or Windows 11, which includes the hosts of the
  Arch recipe and of the Debian package; use without a Docker account; the decisions marked
  "No" above; the alpha 8 red-team evidence.
- **Reversed:** the owner's direction of 2026-11-05 on #546, to own the sandbox and traffic
  control inside Torlek, taken after the same costs were paid for two external engines:
  pins, a separate release train and behaviour Torlek could not change.
- **Cost:** B's 9 to 11 weeks, plus 2 to 3 weeks of removal and migration, plus a full
  red-team run before #833 can turn any sandbox on by default.

## Recommendation

**A.** The native sandbox met the alpha 8 bar on the platforms Torlek ships, and its
remaining work is small and filed. B costs nine to eleven weeks to serve, on Linux, hosts the
native sandbox already serves, and C trades the owner's decisions and most of the supported
hosts for a stronger kernel boundary that the threat model does not ask for.

Docker Sandboxes is the best candidate found so far for the platforms Torlek cannot sandbox.
When the macOS roadmap is scheduled, a time-boxed prototype should answer the two questions
this research could not: whether the agent socket can cross the VM with an identity the
protocol rules accept, and whether a required Docker sign-in is acceptable for an optional
backend. Until then nothing is built.

## Decisions for the owner

**Q1. Replace Torlek's own sandbox and container crates with Docker Sandboxes (option C)?**

- **A (recommended).** No. Torlek's native sandbox stays the sandbox on Linux and WSL2.
- **B.** Yes. Plan the replacement; #833 waits for it.

**Q2. Build a Docker Sandboxes backend beside ours on Linux now (option B)?**

- **A (recommended).** No. Nothing is built; #833, #1118 and #834 continue on the native
  sandbox.
- **B.** Yes. File the backend for a milestone, starting with the agent-socket decision.

**Q3. When the macOS roadmap (#568) is scheduled, how is its sandbox chosen?**

- **A (recommended).** Prototype Docker Sandboxes first, time-boxed to two weeks (one harness launched, and
  the agent socket carried across the VM, without tests or polish), as an
  optional backend for macOS and native Windows only, and decide from what it shows. This
  accepts that the backend would need a Docker account.
- **B.** Rule Docker Sandboxes out now: Torlek's sandbox is Torlek's own on every platform,
  and macOS waits for a native design.

## Sources, limits and what was not checked

- Docker: the pages and quotes in [sources.md](sources.md), read on 2026-11-24. Docker's
  documentation pages carry no revision date; the release notes, the blog posts and the CVE
  records are dated.
- Tracker reports are one person's observation each, on the version they name, and some are
  months old. Replies are called Docker's only where GitHub marks the author a member of the
  `docker` organization.
- Torlek: the three crate READMEs and their sources at `09335ceb`; the reports for
  [#546](../2026-11-05-546-own-sandbox-and-traffic-control.md),
  [#856](../2026-11-05-856-docker-in-sandboxed-sessions.md),
  [#857](../2026-11-05-857-forge-credentials-and-push-policy.md),
  [#858](../2026-11-05-858-provider-sign-ins-and-the-sandbox.md),
  [#860](../2026-11-05-860-host-service-forwards.md) and
  [#861](../2026-11-05-861-unadapted-programs-and-adopted-sessions.md); issues #685, #1158,
  #1387 and #833 with their comments.
- Nothing was run. No start time, memory use or file speed of a Docker sandbox was measured,
  and no claim of Docker's was tested, including the ones this report relies on most: that
  nothing outside the workspace is visible and that all TCP leaves through the proxy.
- Not checked: whether `sbx` works on Arch or inside WSL2 in practice (Docker does not
  support them; a user reports Arch, and Docker's pages mention WSL2 setups); whether a read-only mount of `.git/hooks` inside a
  writable workspace is accepted; whether an HTTP path rule can block a push; whether the
  Unix socket relay can carry Torlek's agent socket; the fixed version of four CVEs whose
  records name none; the cloud sandboxes, which this report leaves out.
- Docker Sandboxes changes fast. A reader after `sbx` 0.54.0 should read the release notes
  before relying on a row above.
