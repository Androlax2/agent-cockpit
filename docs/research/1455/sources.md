# Sources for #1455

Appendix to [the Docker Sandboxes report](README.md), for a reader checking a claim. Every
page was read on 2026-11-24. Nothing was installed or run and no Docker account was created.
Docker's documentation pages carry no revision date, so a later reader may find different
text; the release notes, blog posts and CVE records are dated.

## How the quotes were checked

Each sentence quoted in the report was found again, word for word, in a copy of its page
fetched on 2026-11-24. Figures from issue reports were read from the issue body through
GitHub's API. Two kinds of statement were not checked that way and are marked in the report:
*inference* and *reported*.

## Docker's documentation

| Page | What the report takes from it |
| --- | --- |
| [Docker Sandboxes](https://docs.docker.com/ai/sandboxes/) | The product's scope; `sbx` and the removed Docker Desktop plugin |
| [Install](https://docs.docker.com/ai/sandboxes/install/) | Platforms: macOS 14 on Apple silicon, Windows 11 on x86-64, Ubuntu 24.04 or later with KVM. "You don't need Docker Desktop or Docker Engine to use `sbx`." "Docker does not test or support Docker Sandboxes on Ubuntu derivatives" |
| [Get started](https://docs.docker.com/ai/sandboxes/get-started/) | Later runs "start in seconds" |
| [Architecture](https://docs.docker.com/ai/sandboxes/architecture/) | The microVM, the `sandboxd` daemon, the MCP gateway. "Sandboxes trade higher resource overhead (a VM plus its own daemon) for complete isolation" |
| [Security](https://docs.docker.com/ai/sandboxes/security/) | "The primary trust boundary is the microVM. The agent has full control inside the VM, including sudo access." "local stdio MCP servers run on the host, not inside the sandbox VM" |
| [Isolation](https://docs.docker.com/ai/sandboxes/security/isolation/) | The workspace mount, clone mode, the private Docker Engine, the clipboard. "There is no boundary between the agent's edits and your host filesystem." Clone mode protects the repository "from modification, **not from inspection**". "The agent has no path to your host Docker daemon" |
| [Defaults](https://docs.docker.com/ai/sandboxes/security/defaults/) | The Open, Balanced and Locked Down presets and the Balanced baseline |
| [Credentials](https://docs.docker.com/ai/sandboxes/configuration/credentials/) | The host proxy "overwrites the auth header"; the eleven built-in services; "SSH agent forwarding is enabled by default. Private keys stay on the host, but any process inside the sandbox can ask the forwarded agent to authenticate or sign data" |
| [Settings](https://docs.docker.com/ai/sandboxes/configuration/settings/) | `ssh.agentForwardingEnabled`, `browser.enabled`, `telemetry.disabled`, memory and CPU defaults |
| [Usage](https://docs.docker.com/ai/sandboxes/usage/) | Extra mounts, disks, lifecycle. "Updating the `sbx` CLI or pulling an updated template doesn't update agents inside existing sandboxes" |
| [Agents](https://docs.docker.com/ai/sandboxes/agents/), with the [Claude Code](https://docs.docker.com/ai/sandboxes/agents/claude-code/), [Codex](https://docs.docker.com/ai/sandboxes/agents/codex/) and [OpenCode](https://docs.docker.com/ai/sandboxes/agents/opencode/) pages | The agent list; the flags each agent starts with |
| [Git workflows](https://docs.docker.com/ai/sandboxes/workflows/git/) | Direct and clone mode; a host worktree mounted directly has no Git access |
| [Automation](https://docs.docker.com/ai/sandboxes/workflows/automation/) | Headless use, `sbx policy init`, sign-in with a personal access token |
| [FAQ](https://docs.docker.com/ai/sandboxes/faq/) | "Signing in gives each sandbox a verified identity"; free "including for commercial and professional work"; telemetry |
| [Troubleshooting](https://docs.docker.com/ai/sandboxes/troubleshooting/) | The `forward`, `forward-bypass` and `transparent` egress paths and which of them terminates TLS |
| [Release notes](https://docs.docker.com/ai/sandboxes/release-notes/) | Version 0.54.0 (2026-11-16); the breaking changes of 0.48.0, 0.51.0, 0.53.0 and 0.54.0; Gemini retired in 0.53.0 |
| [Local governance](https://docs.docker.com/ai/governance/local/) and its [approvals](https://docs.docker.com/ai/governance/local/approvals/) page | "All outbound TCP traffic from the sandbox routes through a proxy on your host." "The request that triggered the approval doesn't wait for your answer. It's denied." An approval covers one destination and port for one sandbox |
| [Network rules](https://docs.docker.com/ai/governance/local/network-rules/) and their [reference](https://docs.docker.com/ai/governance/reference/network-rules/) | Rule syntax, scopes, deny over allow, and the three cases where "a denied host that shares an IP address with an allowed host can still be reached" |
| [Audit logs](https://docs.docker.com/ai/governance/organization/audit/) | Audit logs belong to the paid plan |
| [`sbx` command reference](https://docs.docker.com/reference/cli/sbx/) | `run`, `create`, `exec`, `policy`, `policy log`, `policy approval respond`, `port publish`, `inspect`, `cp` and their `--json` output |
| [Sandboxes API](https://docs.docker.com/ai/sandboxes-api/) | The API and SDK are for cloud sandboxes |
| [Docker Desktop release notes](https://docs.docker.com/desktop/release-notes/) | 4.58.0 (2026-01-26) and the plugin's removal in 4.80.0 (2026-06-29) |
| [Security announcements](https://docs.docker.com/security/security-announcements/) | The 0.42.0 fixes for CVE-2026-77179 and CVE-2026-79994 |

## Docker's other pages

| Page | What the report takes from it |
| --- | --- |
| [`docker/sbx-releases`](https://github.com/docker/sbx-releases) | Binaries only. The README says "Proprietary — Docker Inc."; the LICENSE file is "Copyright © 2026 Docker Inc. All rights reserved." A Rocky Linux 8 package and a generic Linux archive beside the Ubuntu packages |
| [Pricing](https://www.docker.com/pricing/) | Local sandboxes at no charge; cloud sandboxes from $0.07 an hour; AI Governance priced per organization |
| [Subscription Service Agreement](https://www.docker.com/legal/docker-subscription-service-agreement/) | The terms that cover the locally installed software |
| [A New Approach for Coding Agent Safety](https://www.docker.com/blog/docker-sandboxes-a-new-approach-for-coding-agent-safety/) (2025-11-25) | The first preview "runs agents as containers inside Docker Desktop's VM" |
| [Why MicroVMs](https://www.docker.com/blog/why-microvms-the-architecture-behind-docker-sandboxes/) (2026-04-16) | "we built a new VMM"; Hypervisor.framework, Windows Hypervisor Platform and KVM |
| [Docker Sandboxes vs. Containers](https://www.docker.com/blog/docker-sandboxes-vs-containers/) (2026-11-05) | "KVM on supported Ubuntu systems, and running inside another VM requires nested virtualization" |

## CVE records

From the [National Vulnerability Database](https://nvd.nist.gov/vuln/search), keyword
"Docker Sandboxes", read 2026-11-24. All twelve were filed by Docker's own CVE authority;
the scores are Docker's CVSS 4.0 and the database had analysed none of them yet. The records
of CVE-2026-18171, -101998, -105452 and -105570 name no fixed version.

| CVE | Published | Score | Summary of the record |
| --- | --- | --- | --- |
| [CVE-2026-12039](https://nvd.nist.gov/vuln/detail/CVE-2026-12039) | 2026-06-18 | 5.7 | The allow-list was not applied to name resolution, so data could leave through DNS |
| [CVE-2026-12539](https://nvd.nist.gov/vuln/detail/CVE-2026-12539) | 2026-06-18 | 5.7 | The ICMP block was not applied again after a daemon restart |
| [CVE-2026-18171](https://nvd.nist.gov/vuln/detail/CVE-2026-18171) | 2026-08-12 | 5.7 | A host folder attached read-only stayed writable at its shared path |
| [CVE-2026-77179](https://nvd.nist.gov/vuln/detail/CVE-2026-77179) | 2026-09-15 | 9.4 | macOS: the file server followed a symlink; a guest could "escape the shared workspace, and read or modify arbitrary host files" |
| [CVE-2026-79994](https://nvd.nist.gov/vuln/detail/CVE-2026-79994) | 2026-09-15 | 8.7 | "The guest-to-host Unix-domain socket relay" could be made to connect to a socket outside the workspace |
| [CVE-2026-101998](https://nvd.nist.gov/vuln/detail/CVE-2026-101998) | 2026-10-08 | 5.9 | Credential masking in proxy responses could fail open |
| [CVE-2026-105452](https://nvd.nist.gov/vuln/detail/CVE-2026-105452) | 2026-10-08 | 5.9 | The proxy forwarded a credential supplied by the sandbox beside the injected one |
| [CVE-2026-105570](https://nvd.nist.gov/vuln/detail/CVE-2026-105570) | 2026-10-08 | 6.7 | A host name in another letter case bypassed token masking |
| [CVE-2026-103916](https://nvd.nist.gov/vuln/detail/CVE-2026-103916) | 2026-11-03 | 8.4 | `sbx cp` followed a workspace symlink to a host path outside the workspace |
| [CVE-2026-121614](https://nvd.nist.gov/vuln/detail/CVE-2026-121614) | 2026-11-03 | 7.1 | Host-side Git commands ran a program named by the repository's configuration |
| [CVE-2026-126329](https://nvd.nist.gov/vuln/detail/CVE-2026-126329) | 2026-11-12 | 7.3 | A daemon run as root trusted state folders it did not own |
| [CVE-2026-126330](https://nvd.nist.gov/vuln/detail/CVE-2026-126330) | 2026-11-12 | 2.4 | A removed port mapping could be published again |

## Docker's issue tracker

Issues of [`docker/sbx-releases`](https://github.com/docker/sbx-releases/issues), read through
GitHub's API. "Maintainer" means GitHub marks the author a member of the `docker`
organization; every other entry is a user's report.

| Issue | State | What the report takes from it |
| --- | --- | --- |
| [5](https://github.com/docker/sbx-releases/issues/5) | closed | Maintainer, 2026-04-23: "The login requirement is purely a business reason" |
| [321](https://github.com/docker/sbx-releases/issues/321) | closed, not planned | Maintainer, 2026-07-09: "There are currently no technical reasons for the login"; "we currently have no plans to change this" |
| [609](https://github.com/docker/sbx-releases/issues/609) | open | Report, 2026-09-21: a signed-in `sbx` refused to work while `login.docker.com` could not be reached |
| [884](https://github.com/docker/sbx-releases/issues/884) | open | Reply, 2026-11-18: "we're actually working on a supported local api along with sdk updates" |
| [106](https://github.com/docker/sbx-releases/issues/106) | open | Report, 2026-05-06: an Arch package recipe built from the generic archive. Reply, 2026-10-23: "Debian 13 works as of v0.47.0" |
| [121](https://github.com/docker/sbx-releases/issues/121) | open | Report, 2026-05-05: the forwarded SSH agent authenticated to a code host with no secret configured. Maintainer, 2026-06-23: "we'll make it configurable (per-sandbox opt-in)" |
| [261](https://github.com/docker/sbx-releases/issues/261) | open | Report, 2026-06-22: a built-in agent's allowed hosts stay reachable under a deny-all default. Maintainer: "either a bug, or at the very least, currently undefined behavior" |
| [176](https://github.com/docker/sbx-releases/issues/176) | closed | Report, 2026-05-28: only credential hosts are decrypted, by "Docker Sandboxes Proxy CA" |
| [31](https://github.com/docker/sbx-releases/issues/31) | open | Report, 2026-04-09, macOS: a Maven build at 13:12 on the mount against 2:39 on the sandbox's own disk |
| [662](https://github.com/docker/sbx-releases/issues/662) | open | Report, 2026-10-04, macOS, version 0.46.0: `cp -a` on the mount about 53 times slower than in `/tmp` |
| [828](https://github.com/docker/sbx-releases/issues/828) | closed | Report, 2026-11-06, Ubuntu 24.04, version 0.51.0: an idle sandbox at 1.4 GB of host memory; "Every sandbox defaults to about half of host RAM" |

## Independent review

[Sandboxing coding agents](https://kudelskisecurity.com/research/sandboxing-coding-agents),
Kudelski Security, 2026-11-23, on `sbx` 0.50.0. It scores Docker Sandboxes 4.5 out of 5 and
states its limits: "We did not try to escape the sandbox of each of these tools"; "It's
closed source and the GitHub repository only contains binary releases"; "A Docker account is
required to use Docker Sandboxes"; "Filesystem logs are not supported yet".

## Torlek

- Code at commit `09335ceb`: the READMEs and sources of `crates/cockpit-sandbox`,
  `crates/cockpit-sandbox-launcher` and `crates/cockpit-containers`. Line counts are
  `wc -l` over each crate's `src/` and `tests/`; test counts are `#[test]` and
  `#[tokio::test]` functions.
- Owner decisions: the reports for
  [#546](../2026-11-05-546-own-sandbox-and-traffic-control.md),
  [#856](../2026-11-05-856-docker-in-sandboxed-sessions.md),
  [#857](../2026-11-05-857-forge-credentials-and-push-policy.md),
  [#858](../2026-11-05-858-provider-sign-ins-and-the-sandbox.md),
  [#860](../2026-11-05-860-host-service-forwards.md) and
  [#861](../2026-11-05-861-unadapted-programs-and-adopted-sessions.md), and issue
  [#1158](https://github.com/Androlax2/agent-cockpit/issues/1158).
- The security bar: the alpha 8 red-team run recorded on
  [#685](https://github.com/Androlax2/agent-cockpit/issues/685) on 2026-11-23 and 2026-11-24,
  with its findings #1448, #1449 and #1450.

## What was looked for and not found

- Source for `sbx`, `sandboxd`, the proxy or the virtual machine monitor, or a plan to
  publish it.
- An independent escape test or audit of Docker Sandboxes.
- A compatibility promise for the command line or its JSON output, or a general-availability
  announcement.
- Figures from Docker for start time, memory at rest or file speed, and any file benchmark on
  a Linux host.
- A statement on whether hosted CI runners can run local sandboxes.
- Terms for shipping `sbx` with another product. An orchestrator can call a user's installed
  `sbx`; the licence grants no right to redistribute it.
