# Forge credentials and push policy with proxy-side credentials

Issue [#857](https://github.com/Androlax2/agent-cockpit/issues/857), a decision issue for the
sandbox stage (alpha 4). **Research and proposal, 2026-11-05; Theo's decisions are recorded under
[Decisions for the owner](#decisions-for-the-owner) as they are made.** Merging this document
accepts research delivery only. It authorizes no product code, dependency, host setting or issue
edit; the issue text under [Changes for each linked issue](#changes-for-each-linked-issue) is
proposed, and the owner applies it. Background: the
[#546 report](2026-11-05-546-own-sandbox-and-traffic-control.md) and its decisions D1 to D6.

## The question

[#155](https://github.com/Androlax2/agent-cockpit/issues/155) has Git inside the sandbox use "a
Cockpit credential helper over the agent channel", with the daemon holding the repo-scoped token
and applying the push policy. [#832](https://github.com/Androlax2/agent-cockpit/issues/832) adds
the credential at the proxy instead, on the wire, for hosts with a bound credential. Building both
means one is rebuilt. Three things to settle:

1. Who authorizes a push, and where the push policy runs.
2. Where [#694](https://github.com/Androlax2/agent-cockpit/issues/694)'s pre-push secret scan runs.
3. Which forges and remote kinds can bind a credential, and whether the project's allowed hosts
   ([#153](https://github.com/Androlax2/agent-cockpit/issues/153),
   [#60](https://github.com/Androlax2/agent-cockpit/issues/60)) are the same list as
   [#674](https://github.com/Androlax2/agent-cockpit/issues/674)'s network grants (D4).

## What the code does today

- A project's sessions get its fine-grained token as `GH_TOKEN` in their environment (#754;
  `crates/cockpit-config/src/github_token.rs`, `crates/cockpit-sessions/src/lifecycle.rs:2159`).
  It passes through the engine unchanged because Git sends it inside Basic credentials, which
  Greyproxy cannot swap
  ([decision](../decisions/2026-11-04-754-per-project-github-token.md)). The token's scope is the
  only bound on what a session that reads it can do. #832 replaces this pass-through.
- The daemon already has a push path: `cockpit_projects::push`
  (`crates/cockpit-projects/src/push.rs`). Current branch only, `refs/heads/B:refs/heads/B` with no
  `+`, one `origin` URL that must be a GitHub URL (`git@github.com:`, `ssh://git@github.com/` or
  `https://github.com/`), redirects refused, host Git with `SAFE_CONFIG` (hooks off,
  `credential.helper=` reset). It is owner-only: `Command::CheckoutPush` is answered
  `owner_only()` to a sandboxed agent (`crates/cockpit-daemon/src/routes.rs:306`). No agent-channel
  verb pushes, and the daemon holds no per-project credential.
- The daemon can already list what a checkout changed outside the default branch, for the
  approvals' build-file warning (#156; `Git::changed_files`).
- The profile denies `~/.git-credentials`, `.git/hooks`, `.git/config` and `~/.gitconfig`
  (`crates/cockpit-sandbox/src/profile.rs:24,37`); #754 also hides `~/.config/gh` once a project
  has a token.
- There is no forge-api crate yet (#60, #153); the push rule's "both on GitHub" is hard-coded in
  `is_github_url`.

## Facts that shape the options

1. **A Git credential helper cannot apply a push policy.** Git calls a helper with the protocol,
   host and (optionally) path before it has negotiated any reference. The helper never sees the
   branch, a force, a tag, a delete, the files changed or the commits. #155's "the daemon applies
   the push policy before answering" cannot be done at that point; the most a helper can do is
   refuse or grant the token for a whole host. The hybrid the issue asks about ("helper for policy,
   proxy for the token") therefore does not work as worded: the policy needs a place that sees the
   refs.
2. **A helper hands the token to the sandbox.** Whatever a helper returns enters Git's process and
   can be requested again by the agent (`git credential fill`, or calling the helper directly). "A
   sandboxed agent cannot read the token" (#155's acceptance) holds only for a design where the
   token never crosses into the sandbox: the proxy (#832) or the daemon.
3. **A proxy sees HTTP, not Git.** After interception it can read the method, path and headers, and
   for `git-receive-pack` the reference commands at the start of the body (old id, new id, name):
   enough for branch patterns, tags and deletes. It cannot tell a force from a fast-forward (that
   needs the commit graph), it cannot see the files changed or run a secret scan without unpacking
   the pack (a Git parser in the process that holds the CA key), and it cannot serve an SSH remote
   at all.
4. **Any credential with write access to the repository, reachable from the sandbox by any path,
   voids the push policy.** The forge's REST and GraphQL APIs create refs and commits
   (`git/refs`, `contents`) with the same Contents write permission. A policy and a scan on the Git
   path mean nothing if the API host carries a token that can write contents.
5. **The forge can enforce part of the policy by itself.** A fine-grained GitHub token without the
   Workflows permission cannot push a change to `.github/workflows` (#754 already asks for "no
   workflow edits"), and a branch ruleset (#382, #695) stops force pushes and deletes of the
   default branch whatever the client sends. Torlek's own check adds the owner approval and the
   typed refusal; it is not the only lock.
6. **This project's own workflow pushes with a lease.** `CLAUDE.md` lets a session push its
   `greenfield/<issue>-<slug>` branch with `--force-with-lease` after a rebase. #155's "never
   force" would break that. The policy needs a named exception, or the workflow changes.

## Options for push authorization and policy

| | Token reaches the sandbox | Policy can see | Remote kinds | Cost |
| --- | --- | --- | --- | --- |
| **A. Credential helper over the agent channel** (#155 as written) | Yes, on every request | Host and path only | HTTPS | Small; policy is in name only |
| **B. Proxy only** (#832 carries pushes) | No | Ref names, tags, deletes; not force, files, secrets | HTTPS forges the proxy can intercept | A pack parser in the launcher, or no scan and no workflow check |
| **C. Daemon pushes; proxy holds a credential with no Contents write** (recommended) | No: the push token stays in the daemon, the proxy credential cannot write contents | Everything: the daemon has the checkout, the commits and the real refs | Any URL the push rule accepts, including SSH | A new agent-channel verb; two credentials per project |
| **D. Daemon pushes; no credential at the proxy** | No | Everything | Same as C | Same verb, but agents lose `gh` and private fetch; every forge action becomes a `torlek` verb (#60) |

### Threats each option leaves

| Threat | A | B | C | D |
| --- | --- | --- | --- | --- |
| Agent reads the push token (environment, memory, `git credential fill`) | Open | Closed | Closed | Closed |
| Push to a protected or out-of-pattern branch, tag or delete | Open | Closed by ref names | Closed | Closed |
| Force push over history | Open | Open (forge ruleset only) | Closed by refspec and lease rule | Closed |
| Change to a workflow file | Forge token scope only | Forge token scope only | Owner approval, plus token scope | Same |
| Secret in a pushed commit (#694) | Open | Open (no scan without a parser) | Closed: scan on the exact pushed range | Closed |
| Merge a pushed branch into the default branch through the API (Pull requests write) | Open | Open | Open: only the forge ruleset and CI stop it | Closed: no credential |
| Write through the forge API instead of Git (fact 4) | Open | Open | Closed only if the API credential has no Contents write | Closed: no credential |
| Exfiltration through an allowed forge host with the credential | Open | Open | Open: bounded by the API token's scope | Closed for the forge |
| Compromised launcher exposes the credential and the CA key | n/a | Both | The API credential and the CA key | The CA key only if another host has one |
| Agent-written objects parsed by the daemon's Git | None | None | Open: the existing host-Git hazard, bounded by `SAFE_CONFIG` and the explicit refspec | Same |
| Owner's own SSH key used for the push | n/a | n/a | Open until a per-project key exists | Same |

The merge row is a limit of C worth stating plainly: opening and merging pull requests is the
intended path (`CLAUDE.md` arms `gh pr merge --auto --squash`), so the API credential must carry
Pull requests write, and a pull request can land content on the default branch or a `release/*`
branch that `torlek push` refuses to push to. Torlek's push policy does not govern that; the
forge's ruleset (pull request and green CI required, no bypass; #382) does, and `torlek doctor`
reporting an unprotected default branch (#695) becomes part of the security story, not an
extra. A project without such a ruleset lets its agent merge its own pull request.

The "Closed" cells above hold only with `agents.sandbox = "required"`. Until the native sandbox is
the default (#546, stages 1 to 5) sessions run unsandboxed, keep the pass-through `GH_TOKEN` of
the issue 754 pass-through, and a plain `git push` bypasses the verb, the policy and the scan; the owner already accepts
that interval.

### Recommendation: C

The daemon pushes, because it is the only place that sees the real refs, the commit graph and the
changed files, and the push token never leaves it. The sandbox keeps the proxy credential of #832,
narrowed to what a session needs without writing contents.

- **Push verb.** A new agent-channel verb, `torlek push`, under a new grant (`push-branch`, one
  term per branch pattern) checked like the other verbs. It runs the existing
  `cockpit_projects::push` path with these additions, each a fixture-tested refusal:
  - resolve the branch to one commit id first and push `<id>:refs/heads/B`, so the scan, the
    checks and the push see the same commit (no race with an agent that moves the branch);
  - branch patterns from the project policy (default: anything except the default branch and
    `release/*`); never a tag; never a delete;
  - force: none, except the lease rule of [decision 2](#decisions-for-the-owner): a lease-protected
    force on the session's own branch, expected tip = the commit the daemon last pushed there;
  - a change to `.github/workflows` or `.gitlab-ci.yml` in the pushed range (`git diff
    --name-only` between the range base and the resolved commit id; not `Git::changed_files`,
    which reads the working tree and `HEAD` against `origin/HEAD`) becomes an owner approval
    request, as `ask run` does;
  - the pushed range is the commits the push adds: from the remote branch tip when the push
    is a fast-forward, otherwise (new branch, or a lease-forced rewrite) from the merge base with
    the default branch, so a rebased branch is scanned as a whole and never against a tip that
    is not its ancestor;
  - the pre-push secret scan and the trailer check of #694 on that range;
  - the lease needs state that does not exist yet: the daemon records, per checkout and branch,
    the commit it last pushed (where it is stored is for the implementing issue). With no record
    (restart without it, a re-clone, a branch the owner or an older build pushed) the force is
    refused with that reason, never replaced by the remote's current tip, which would make the
    lease an unconditional force;
  - every refusal typed, shown to the agent and logged with the reason.
- **Push credential.** The project's existing #754 token file stays the push token and is read by
  the daemon only. It stops being injected as `GH_TOKEN` when the native sandbox is required
  (before that, sessions run unsandboxed and nothing hides it). An SSH origin is
  out of the first release ([decision 4](#decisions-for-the-owner)): `torlek push` refuses it
  with a named reason, and the owner's own key is never used by the agent verb.
- **Proxy credential (#832).** A second per-project credential, stored like the first and apart
  from it: its own folder of the owner's configuration directory (already denied to agents),
  mode `0600` or `0400`, no symbolic link, one token, read at each launch under the rules of
  `cockpit_config::read_github_token`. Nothing stores one today (#754 knows one token and one
  variable). The daemon hands it to the launcher on the inherited descriptor, never in the
  environment; the sandbox gets a placeholder `GH_TOKEN` so `gh` runs, and the proxy sets the
  `Authorization` header on that host, replacing whatever the client sent (this also removes
  #754's Basic-credentials problem). The credential is for the forge API and for read
  access to Git: Pull requests and Issues write, Contents read, no Contents write, no Workflows,
  no admin. On the Git host the proxy adds it for `git-upload-pack` only and refuses both halves of a push:
  `GET .../info/refs?service=git-receive-pack` and `POST .../git-receive-pack` (fetch uses
  `service=git-upload-pack` on the same `info/refs` path, so the query counts, not the path alone;
  fetch works for a private repository and the sandbox cannot push around the daemon). Whether `gh pr merge --auto` works with that permission set is not verified here;
  the implementing issue must test it on a real repository before relying on it.
- **Where the scan runs.** In the daemon's push verb, on the resolved range, with real Git. Not in
  the proxy and not in the sandbox: neither sees an SSH push, and the proxy would need a pack
  parser in the process that holds the CA key.

## Forges and remote kinds

The table shows what each remote can do in principle; the first release is GitHub only (decision 4). A credential can be bound at the proxy when the remote speaks HTTPS and the proxy can add a header
the forge accepts. Each forge adapter (#153) declares its scheme and its hosts, so no forge name
appears outside its crate.

| Remote | Proxy credential (#832) | Daemon push (C) | Note |
| --- | --- | --- | --- |
| GitHub | Yes: `Authorization` Basic on `github.com`, Bearer on `api.github.com` | Yes | First adapter (#60) |
| GitLab, gitlab.com or self-hosted | Yes: Basic with the token for Git, `PRIVATE-TOKEN` for the API | Yes | A self-hosted instance with a private CA needs the upstream trust set per host |
| Forgejo, Gitea, Codeberg | Yes: token header or Basic | Yes | Same note |
| Plain Git over HTTPS | Only with a Basic credential the owner binds to the host | Yes | No API |
| Plain Git over SSH | No: the proxy cannot add an SSH key | Later, with a per-project key; refused in the first release | Daemon push would be the only way |

Today only GitHub URLs pass `is_github_url`; the other rows need the allowed-host list of #153.
Decided ([decision 4](#decisions-for-the-owner)): only the GitHub row ships in the first
release; every other row is its own adapter issue later, and a project on one of them is refused
with a named reason, never a silent downgrade.

### One host list or two

Issues #153 and #155 say "allowed hosts": where the daemon may push, default the origin's host. #674 says
"allowed hosts" too: where the sandbox may connect. They answer different questions. Recommended: a
single record per project and host (the grant of D4, revocable, audited) with separate
capabilities on it:

- **reach**: the sandbox may connect (#674's grant);
- **credential**: the proxy adds a bound credential on that host (#832); never implied by reach;
- **push**: the daemon may push to a remote on that host (#153's list); never implied by reach.

Binding a credential or a push permission to a host that has no reach grant is refused, so the
lists cannot drift apart, and revoking reach closes the tunnels (#674) and the credential at once.

## Decisions for the owner

- [x] **Decision 1: push authorization and policy.** Decided 2026-11-05: **C**. The daemon pushes
  through `torlek push`; the proxy credential is for the forge API and Git reads, with no Contents
  write.
- [x] **Decision 2: force on a session's own branch.** Decided 2026-11-05: **A**. Lease-protected
  force only on the session's own branch, the expected tip being the last commit the daemon
  pushed there; no other force.
- [x] **Decision 3: one host list or two.** Decided 2026-11-05: one record per project and host
  with separate rights (reach, credential, push). Credential and push are never implied by
  reach.
- [x] **Decision 4: forges and remote kinds in the first release.** Decided 2026-11-05: GitHub
  only. Other forges and SSH get their own adapter issues later and are refused with a named
  reason until then.

## Changes for each linked issue

Proposed text; the owner applies it. Nothing here edits an issue.

### #155 Git safety: push policy and credential broker

Replace the first two Scope bullets and the Acceptance with:

> - **Push verb, not a credential helper (#857, decision 1).** A sandboxed agent pushes with
>   `torlek push` on the agent channel, under a `push-branch` grant with one term per branch
>   pattern. The daemon runs the existing host push (`cockpit_projects::push`) on the session's
>   checkout. There is no credential helper in the sandbox and the push token never enters it.
> - **Policy, each a typed refusal shown to the agent and logged:** the branch resolves to one
>   commit id first and the push is `<id>:refs/heads/B`; branch patterns (default: everything
>   except the default branch and `release/*`); never a tag, never a delete; no force except a
>   lease-protected force on the session's own branch, expected tip = the commit the daemon last
>   pushed there (decision 2); a change to `.github/workflows` or `.gitlab-ci.yml` needs an owner
>   approval; the origin must be a GitHub HTTPS URL whose host the project has a `push` right on
>   (decisions 3 and 4); any other remote is refused naming why ("SSH remotes are not supported
>   yet", "no adapter for this forge").
> - **Acceptance:** fake-forge and real-Git tests for each refusal, including a branch moved
>   between the scan and the push; a direct `git push` from inside the sandbox fails; a sandboxed
>   agent cannot read the push token from its environment, files or `git credential fill`; a
>   rebased branch is scanned from the merge base; a lease-force with no recorded last-pushed
>   commit is refused. `CLAUDE.md`'s Git authorization is reworded in the same pull request.
>
> **Depends on:** #28, #32, #60 (adapter), #857 (decision). Split with #694 (scan, trailer) and
> #695 (protect) as before.

### #694 Agent commits carry a trailer; pre-push secret scan

Add under Scope:

> The scan and the trailer check run inside `torlek push` (#155), on the range from the remote
> branch tip (or the default branch) to the commit id the push will send, with real Git, so the
> scan and the push see the same commit. Not in the proxy and not in the sandbox: neither sees the
> files, and the proxy would need a pack parser next to the CA key (#857). A range with a commit
> that lacks the trailer is refused with the commit id.

Acceptance: "Real-Git tests through `torlek push`: ...". Open for the owner when this is built:
whether the scan is gitleaks as a required host program or a bundled ruleset (a dependency
decision, not made here).

### #60 cockpit-forge-api GitHub adapter

Add under Scope:

> The adapter declares the hosts and credential schemes it needs, so the proxy (#832) and the
> daemon push (#155) need no GitHub string: `github.com` (Basic, Git reads only) and
> `api.github.com` (Bearer). "Repo-only tokens" are two per project: the **push token**
> (Contents write, no Workflows, no admin), read by the daemon only, and the **API token**
> (Pull requests and Issues write, Contents read only, no Workflows, no admin), added by the
> proxy. A scope check reports each as verified or unverified where the forge cannot tell.

### #153 cockpit-forge-api: forge-neutral

Replace the push-policy bullet with:

> `cockpit-projects` push policy: replace "both on GitHub" with the project's host records
> (#857, decision 3): one record per project and host with the rights reach, credential and push,
> default the origin's host with reach and push only after the owner binds it. An adapter declares
> its hosts and credential schemes. First release: GitHub only (decision 4); a project whose
> origin needs another adapter, or an SSH URL, is refused with a named reason. GitLab, Forgejo
> and plain Git over SSH are separate adapter issues, filed when wanted.

### #829 native Linux sandbox launcher

Add to Acceptance:

> - With a project token configured and `agents.sandbox = "required"`, the agent's environment
>   holds no push token (no `GH_TOKEN` carrying Contents write), `~/.config/gh` is unreadable, and
>   a direct `git push` from inside fails (#857).

### #831 per-session egress proxy

Add:

> Allowlist entries are the `reach` right of a host record (#857, decision 3); a bound credential
> or a push right is a separate right on the same record and is never implied by an allow. Until
> #832, a tunnel to a forge host carries no credential.

### #832 selective TLS interception for hosts with a bound credential

Replace the Acceptance with:

> - A host with a bound credential is intercepted with a per-session CA whose key never leaves
>   the launcher's memory; only the certificate enters the session, with trust variables per
>   runtime. The credential is the project's **API token** (no Contents write, #857 decision 1),
>   never the push token, and is added on its own host only.
> - On `github.com` the proxy adds it for `git-upload-pack` only and refuses both
>   `GET .../info/refs?service=git-receive-pack` and `POST .../git-receive-pack` with a reason
>   naming `torlek push`, one test for each, and a fetch test on the same `info/refs` path; `api.github.com` gets it as
>   Bearer. Private fetch, `gh issue view` and `gh pr create` work with no token inside; whether
>   `gh pr merge --auto` works with this permission set is tested on a real repository first.
> - Binding a credential to a host without the `reach` right is refused (decision 3). Every other
>   host stays a plain tunnel; a pinned client's failure names the host.
> - This replaces #754's `GH_TOKEN` pass-through: with the native sandbox required, no launch
>   gives the sandbox a token with Contents write.

### #674 refuse at once, owner decides

Add:

> Host records (#857, decision 3): "allow always" creates or extends the project's `reach` right
> for that host and port only. `credential` and `push` rights are never created by a network
> request decision; the owner binds them explicitly, and revoking `reach` closes open tunnels and
> removes the host's other rights.

### Repository instructions

`CLAUDE.md`'s Git authorization ("push its own branch ... `--force-with-lease`") describes a
direct `git push` and a `GH_TOKEN` in the environment. When #155 and #832 ship, a sandboxed
session pushes with `torlek push` and `gh` runs on a placeholder token, so that paragraph is
reworded in the pull request that ships them (add it to the acceptance of #155). Not before: it
describes the unsandboxed interval correctly today.

### Not changed

Issue #695 (protect) builds on the adapter as written. #754 is closed; its token file stays the push
token, read by the daemon, and its pass-through ends with #832. #59 keeps accounts and quota;
its proxy placeholders go to #832 as #546 already says.
