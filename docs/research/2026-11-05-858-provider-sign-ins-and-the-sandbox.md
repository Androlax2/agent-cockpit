# Provider sign-ins and account credentials in the sandbox

Decision issue [#858](https://github.com/Androlax2/agent-cockpit/issues/858), covering
[#59](https://github.com/Androlax2/agent-cockpit/issues/59). It builds on the
[native sandbox plan](2026-11-05-546-own-sandbox-and-traffic-control.md) and its decisions
(cited as "#546 D1" to "#546 D6"), on the [folders and caches decision](2026-11-05-859-session-folders-and-caches.md)
("#859 Q1" to "#859 Q3") and on the [forge credentials decision](2026-11-05-857-forge-credentials-and-push-policy.md)
("#857 decision 1" to "4"). **Research and proposal; Theo's answers are recorded under
[Decisions for the owner](#decisions-for-the-owner) as they are made.** Merging this document
accepts research delivery only: it authorizes no product code, dependency, host setting or issue
edit. The [issue texts](#changes-to-the-linked-issues-proposed-text) at the end are proposals;
Theo applies them.

## The question

A provider harness (Claude Code, Codex, OpenCode) signs in once and keeps the result in a file
under its own folder. #830 declares each provider's folders as paths inside the sandbox, so as
planned the sign-in would go in with them. #59 plans "proxy placeholders" for accounts and a
switch of account at a quota stop. Should provider credentials be bound at the egress proxy,
like the project's `GH_TOKEN` under #832, or mounted per account at launch? How does a
quota-stop switch work then, and what does #830 declare?

### Already decided elsewhere

- **#859 Q3 (Theo, 2026-11-05):** no tool configuration or credential file of the home is
  mounted; other folders only by a per-project grant the owner decides, read-only unless the
  grant says write; never a path of `BASELINE_DENIED_READ` or `BASELINE_DENIED_WRITE`. Theo's
  comment on #858 applies this to provider folders: the owner's `~/.claude`, `~/.codex` and
  OpenCode data are not mounted, and #830's per-provider folder list is where this decision
  lands. An earlier draft of this report asked whose provider folders a session gets; #859
  answers it.
- **#857 decision 3:** one record per project and host with separate rights (reach, credential,
  push); a credential is never implied by reach, and binding one to a host with no reach is
  refused.
- **Report 18 (2026-10-31):** an account is a credential reference; routing happens at launch
  and at a quota-stop restart only.

## Recommendation

- [x] Q1 decided 2026-11-05: A, bound at the proxy; a login that refreshes itself only through
  a per-project grant naming the account.
- [x] Q2 decided 2026-11-05: A, relaunch only, as report 18 decided.
- [x] Q3 decided 2026-11-05: A, one account per provider bound at the proxy in alpha.4; several
  accounts, login grants and routing stay in #59.

Theo chose the recommended answer on all three questions:

1. **Every provider credential is bound at the proxy** when the harness accepts it from its
   environment: API keys for every provider, and Claude subscriptions through a
   `claude setup-token` token. The session holds a placeholder; the launcher adds the real value
   on that provider's API host only (#832's mechanism). No provider credential file is mounted.
2. **A login that refreshes itself** (Codex's ChatGPT plan, OpenCode's OAuth logins, a Claude
   login kept for Remote Control) reaches a session only through a **per-project grant naming the
   account**, which the owner decides and can revoke, as #859 Q3 allows for extra folders. The
   grant mounts that account's login file from a folder Torlek owns, never the owner's own.
3. **Switch accounts only by relaunching**, as report 18 decided: the daemon picks the account
   from its own records and resumes the conversation, which lives in the session's provider
   folder whatever the account.
4. **Alpha.4 binds one account per provider at the proxy**, so sandboxed sessions can sign in
   when #833 turns the sandbox on; several accounts and routing stay in #59 (beta.2).

## What exists today

Code at this branch's base (`26e6e35b`); paths are relative to the repository root. Credential
files on the research host were inspected for their key names and modes only, never their
values.

| Where | Fact |
| --- | --- |
| `crates/cockpit-daemon/src/serve.rs:29` | Every agent inherits the daemon's `HOME`, so each harness finds the owner's own provider folders |
| `crates/cockpit-sandbox/src/profile.rs:17` | The read baseline hides SSH, GPG, cloud, Docker and Git credentials and keyrings; no provider credential file is in it |
| `crates/cockpit-sandbox/src/profile.rs:83` | A secret crosses as `Placeholder` (swapped by Greyproxy, which is being removed) or `PassThrough` |
| `crates/cockpit-sessions/src/lifecycle.rs:2151` | Only two secrets cross today, both `PassThrough`: the launch token and the project's `GH_TOKEN` (#754); #832 takes the second out |
| `crates/cockpit-claude/src/sign_in.rs:20`, `crates/cockpit-codex/src/sign_in.rs:23`, `crates/cockpit-opencode/src/sign_in.rs:20` | Sign-in readiness runs `claude auth status`, `codex login status`, `opencode auth list` in the daemon's environment, against the owner's folders |
| `crates/cockpit-claude/src/lib.rs:312` | A launch can ask for Claude's `--remote-control` (capability `RemoteControl`) |
| `crates/cockpit-provider-api/src/capabilities.rs:31` | `Billing` has no method yet: "account routing (report 18) brings it" |
| `crates/cockpit-provider-api/src/lib.rs:165` | The host never reconstructs launch facts "from agent-writable files or agent reports" |
| [Report 18](../design/greenfield/18-usage-quota-and-accounts.md) | `cockpit-credentials` "injects the secret as a proxy placeholder as today" |
| Host, `~/.claude/.credentials.json` (0600) | `claudeAiOauth` with access token, **refresh token**, `expiresAt` and `refreshTokenExpiresAt`, and **`mcpOAuth` tokens for three MCP servers** (Linear, Notion, Atlassian) |
| Host, `~/.codex/auth.json` (0600) | `auth_mode = chatgpt`: ID, access and **refresh** tokens, account ID, `last_refresh` |
| Host, OpenCode `auth.json` (0600) | One API key and three OAuth logins (Anthropic, OpenAI, Google), each with a **refresh** token |

The legacy launchers (kept until the cutover, #66) already split by kind:

| Legacy piece | What it does |
| --- | --- |
| `config/bin/.local/bin/greywall-agents/claude:26` | A credits session gets the Console API key from the keyring as a Greyproxy placeholder; a subscription session uses the mounted `~/.claude` login |
| `config/bin/.local/bin/greywall-agents/opencode:52` | OpenCode's Anthropic key as a placeholder, "so no session reads it (opencode's own credential store, opencode.db, is open to every opencode session)" |
| `config/bin/.local/bin/greywall-agents/launcher-common.sh:368` | Every other session gets no `ANTHROPIC_API_KEY`, whatever the calling environment holds |
| `config/waybar/.config/waybar/scripts/native-subscription.py:67`, `:115` | Subscription routing reads the mounted credentials file to check the account and its capacity, and refuses any override variable |

### What each harness accepts

Versions on the research host: Claude Code 2.1.318, codex-cli 0.166.0, OpenCode 2.2.5.

| Credential | From the environment? | Refreshes itself? |
| --- | --- | --- |
| Anthropic API key | `ANTHROPIC_API_KEY` (sent as `X-Api-Key`) | No |
| Claude subscription, `/login` | No: `.credentials.json` under `CLAUDE_CONFIG_DIR` | Yes, rotating refresh token |
| Claude subscription, `claude setup-token` | `CLAUDE_CODE_OAUTH_TOKEN`: one year, "can only make model requests, so it can't establish Remote Control sessions or fetch claude.ai connectors" ([docs](https://code.claude.com/docs/en/authentication#generate-a-long-lived-token)) | No |
| OpenAI API key | OpenCode: its provider key variables (the legacy launcher passes `ANTHROPIC_API_KEY`, `opencode:52`). Codex: `codex login --with-api-key` stores it in `auth.json`; whether interactive Codex reads it from an environment variable is unproven. If not, the session's own `auth.json` holds only the placeholder | No |
| Codex ChatGPT login | No for personal plans: `auth.json` under `CODEX_HOME`. `--with-access-token` exists for ChatGPT Enterprise workspaces whose admin grants it ([docs](https://learn.chatgpt.com/docs/auth)) | Yes, "refreshes tokens automatically during use" |
| OpenCode OAuth logins | No: `auth.json` | Yes |

A token that refreshes itself is the crux: the harness posts its refresh token to the
provider's token endpoint and writes the new pair back to its file. A proxy can bind it only by
faking that file and rewriting the refresh exchange, an undocumented protocol that changes with
each harness release.

## Threats

The adversary is #546's: the agent and the code it runs, as the owner's user; a kernel exploit
is out of scope.

| # | Threat | With a login file in the session |
| --- | --- | --- |
| T1 | **Theft of a long-lived credential.** The agent reads the refresh token (or a one-year token) and sends it out through an allowed host: the provider's own API, GitHub, a commit, a log line | The account is usable from anywhere until the owner notices and revokes it |
| T2 | **Collateral tokens.** Claude's credentials file also holds MCP OAuth tokens | Linear, Notion and Atlassian access leave with it |
| T3 | **Account tampering.** The agent logs out, signs in to an account of its own, or rewrites the file | Billing lands elsewhere; anything that reads the file to learn the account believes the agent (`lib.rs:165`) |
| T4 | **Use during the session.** The agent spends the account's quota or credits through the provider host | Same with any option: the session needs the account to work |
| T5 | **Rotation races.** Two holders of one refresh token refresh in turn | One of them is signed out: a per-session copy of a login breaks every other holder |

## Options

Costs are rough guesses for one agent with focused tests, as in #546, on top of #831 and #832.

### Q1. Where provider credentials live

Mounting the owner's provider folders, today's shape, is ruled out by #859 Q3.

| Option | Threats left | Cost |
| --- | --- | --- |
| A. **Proxy-bound, logins by project grant (recommended).** Every credential the harness takes from its environment is bound at the proxy; the session holds a placeholder. A login that refreshes itself reaches a session only through a per-project grant naming the account (#859 Q3's "extra only by a project grant"); the grant mounts that account's login file, read-write, from a Torlek-owned folder, and the session's other provider files stay its own | T4 everywhere. T1 to T3 only for a granted login account, in every project granted that account: its sessions share one login, so this is the one owner-granted exception to #859's "nothing a session writes outside its own folders is read by another session". Claude's MCP logins start outside Torlek's folder, but a granted Claude session that signs in to an MCP server writes `mcpOAuth` into the shared file (T2 again). T5 reduced, not avoided: no copy of a login exists, but two sessions refreshing at once can still race | About 3 days: binding provider hosts in #832 (2 days), the login grant (1 day) |
| B. **Proxy-bound only.** As A without the grant: a login that refreshes itself is never offered to a sandboxed session | T4 only. Codex's ChatGPT plan and OpenCode's OAuth logins run only with `agents.sandbox = "off"`; no Claude Remote Control or claude.ai connectors in sandboxed sessions | About 2 days |
| C. **Torlek refreshes.** As B, and for logins that refresh themselves the daemon keeps the refresh token outside and writes only a short-lived access token into the session before it expires | T1 shrinks to the access token's lifetime (hours, unmeasured); T3 and T4 stay | About 2 days per harness for undocumented refresh protocols and client IDs, again after each release that changes them; T5 against the owner's own login of the same account |
| D. **Torlek-owned login folders mounted for every account**, no proxy binding | T1 to T3 for every session; T4 | About 1 day. Contradicts #859 Q3's rule for credentials: listed only as the cheapest alternative |

Every option with proxy binding shares these costs: all model traffic of a bound provider passes
through Torlek's TLS interception (streaming, HTTP/2, WebSocket), each harness needs its
runtime's trust variable for the per-session CA, and each harness must accept a placeholder of
its token's shape (unproven). The adapter's declaration (#830) names which hosts can carry a
provider credential; it grants nothing. Under #857 decision 3 the credential right is the owner's
act of setting the provider credential, recorded with the provider host's reach so that revoking
one ends the other; how that record sits beside #857's per-project host records (one per project,
or one per profile for provider hosts) is #832's to settle.

### Q2. Switching at a quota stop

Report 18 decided the switch happens only at launch and at a quota-stop restart. With proxy
binding the proxy could swap a token mid-session; that is the open choice.

| Option | Threats left | Cost |
| --- | --- | --- |
| A. **Relaunch only (recommended, report 18 as decided).** The daemon picks the next account with capacity from its own records, relaunches with that account's binding (or granted login file) and resumes the conversation, which stays in the session's provider folder. The account a session ran on is the daemon's record, never read back from the session | None new | Part of #59 |
| B. **Hot swap for bound accounts.** The proxy rebinds the next account between requests; granted logins still relaunch | A turn half on one account, half on another; billing and quota attributed per request, not per session | About 1 day more, and report 18's decision reopened |
| C. **No automatic switch.** A quota stop waits; the owner relaunches on another account | None | None; #59's acceptance (resume within a minute) dropped |

### Q3. What alpha.4 ships before #59

Issue #59 is in v0.1.0-beta.2; Theo's rule of 2026-11-05 keeps implementation in its own milestone.
But #833 turns the sandbox on in alpha.4, and under #859 Q3 a sandboxed session then has no
provider credential at all unless alpha.4 brings one.

| Option | Threats left | Cost |
| --- | --- | --- |
| A. **One account per provider bound in alpha.4 (recommended).** #832 binds the provider credential the owner sets for each provider (an API key or a Claude `setup-token` token); several accounts, the login grant and routing stay in #59 | T4. Sandboxed Codex on a ChatGPT plan and OpenCode OAuth wait for #59 (or run with `off`) | About 2 days moved into alpha.4, inside #832 |
| B. **One Torlek-owned login folder per provider mounted in alpha.4**, as a stopgap until #59 | T1 to T3 for every sandboxed session until beta.2; an exception to #859 Q3 | About half a day, then replaced in #59 |
| C. **#833 waits for #59** | The unsandboxed period (#729) lasts until beta.2 | None now |

## Decisions for the owner

**Q1. Where provider credentials live.** **A (recommended):** bound at the proxy; a login that
refreshes itself only through a per-project grant naming the account. **B:** bound at the proxy
only; such logins not offered in sandboxed sessions. **C:** Torlek refreshes OAuth logins
itself. **D:** Torlek-owned login folders mounted for every account. Most secure: B.

**Decided 2026-11-05: A.** Every provider credential the harness accepts from its environment
is bound at the proxy and never enters the sandbox. A login that refreshes itself reaches a
session only through a per-project grant naming the account, which the owner decides and can
revoke; the grant mounts that account's login from a Torlek-owned folder. Theo accepts T1 to
T3 for a granted account, in the projects granted that account.

**Q2. Switching at a quota stop.** **A (recommended):** relaunch only, as report 18 decided.
**B:** hot swap for bound accounts. **C:** no automatic switch. Most secure: A (C equal, at the
cost of #59's acceptance).

**Decided 2026-11-05: A.** An account changes only at launch and at a quota-stop relaunch: the
daemon picks the next account with capacity from its own records, relaunches with that
account's binding or granted login and resumes the conversation. The account a session ran on
is the daemon's record, never read back from the session. No hot swap.

**Q3. Alpha.4 before #59.** **A (recommended):** one account per provider bound at the proxy in
alpha.4. **B:** one login folder per provider mounted as a stopgap. **C:** #833 waits for #59.
Most secure: A.

**Decided 2026-11-05: A.** Alpha.4 binds one account per provider at the proxy (#832): the
credential the owner sets for that provider. Several accounts, login grants and routing stay
in #59 (beta.2). Until then a sandboxed session on Codex's ChatGPT plan or an OpenCode OAuth login
is not available, and a Claude session bound with a `setup-token` token has no Remote Control and
no claude.ai connectors; the owner's `agents.sandbox = "off"` remains the named opt-out.

## Changes to the linked issues (proposed text)

Theo chose the recommended answer on all three questions, so these texts follow his decisions
as written. Nothing here is applied by this report: Theo applies them.

### #59 (covered; rewrite Scope and Acceptance)

> **Scope** (#858, owner decisions 2026-11-05)
>
> - **Accounts.** An account has a provider, a kind and observed quota state. Kind
>   *bound*: a credential the harness accepts from its environment (an API key, a Claude
>   `claude setup-token` token). Kind *login*: a sign-in that refreshes itself (Codex's
>   ChatGPT plan, OpenCode's OAuth logins, a Claude `/login`). The daemon holds every secret
>   outside the sandbox; no account's secret is read from or written to an agent-writable
>   file.
> - **Bound accounts.** At launch #832 binds the account's credential on its provider's API
>   hosts (#830); the session holds a placeholder in the adapter's variable. Alpha.4 already
>   binds one account per provider; this issue adds several.
> - **Login accounts.** Usable in a project only through a per-project grant naming the
>   account, decided by the owner like a network request (#674): who, when and client
>   recorded, revocable, applied at the next launch. The owner signs the account in once
>   through a guided step into a Torlek-owned folder that holds only that login; it is never
>   copied from the owner's own provider folder, so the owner's own sign-in keeps its own
>   refresh tokens. The grant mounts that login read-write (the file, or a folder holding
>   only it if the harness saves by renaming over the file, which fails on a bind-mounted
>   file). Every session on that account shares it, never a copy. Without the grant a launch
>   on that account is refused with the grant to ask for.
> - **Routing.** Only at launch and at a quota-stop relaunch (report 18): the daemon picks the
>   next account with capacity from its own records, relaunches with its binding or granted
>   login and resumes the conversation. No hot swap. The account a session ran on is the
>   daemon's record, never read back from the session.
>
> **Acceptance** (through the daemon, fake providers)
>
> - A quota-stop restart resumes the same conversation on the second account within one
>   minute.
> - On a bound account, the session's environment and files hold only a placeholder, no
>   provider credential file is visible, and the fake provider's API host receives the real
>   credential.
> - A login account in a project without its grant is refused, naming the grant; with the
>   grant the session signs in; after revocation the next launch is refused.
> - Two sessions on one login account both stay signed in when they refresh at the same
>   time, or a second concurrent session on that account is refused with the reason.
> - A refresh inside a session survives in the shared login: the next session on the
>   account starts signed in.
> - A session that rewrites its provider files does not change the account the daemon
>   records for it.
>
> **Depends on:** #58, #830, #832, #674

### #830 (add to the issue)

> Also (#858, owner decisions 2026-11-05): each adapter declares, apart from its other
> folders, its **credential paths** (Claude's `.credentials.json`, Codex's `auth.json`,
> OpenCode's `auth.json` and any other store it knows), the **environment variable** that
> takes a bound credential (`ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` for Claude, the
> provider key variables OpenCode reads; for Codex, the variable it reads if one works, else a
> session-own `auth.json` holding only the placeholder) and which of its **API hosts carry a credential**. No
> credential path of the owner's provider folders is mounted (#859 Q3). The session's
> provider folder (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, OpenCode's data folder) is a Torlek-owned
> folder kept across the session's relaunches, so a quota-stop relaunch resumes the
> conversation; a granted login (#59) mounts only the account's login into it.
>
> Acceptance to add: through the daemon with a fake provider, the owner's
> `~/.claude/.credentials.json`, `~/.codex/auth.json` and OpenCode `auth.json` are not
> visible, and each adapter names its credential variable and credential hosts.

### #832 (add to the issue)

> Also (#858, owner decisions 2026-11-05): provider credentials are bound here, from
> alpha.4: one account per provider, the credential the owner sets for that provider (an API
> key, or a Claude `setup-token` token), on the API hosts #830 declares as credential hosts.
> The session gets a placeholder in the adapter's variable. Several accounts, login grants and
> routing are #59's.
>
> Acceptance to add:
>
> - A fake provider's request to its API host carries the real credential; the session's
>   environment and files hold only the placeholder; the same placeholder sent to any other
>   host carries no credential.
> - Each adapter's conformance test shows its harness accepts the placeholder and the
>   per-runtime trust variable for the per-session CA; a harness that refuses either is
>   named with the reason.
> - A provider with no credential set starts no sandboxed session, and says which credential
>   to set.

### #829 (add to Acceptance)

> - No credential of the owner's provider folders is visible in the session (#858):
>   `~/.claude/.credentials.json`, `~/.codex/auth.json` and OpenCode's `auth.json` are absent
>   from the view, and the environment holds no real provider credential, only #832's
>   placeholders. The one exception, from #59, is the login a per-project grant mounts.

### #674 (add a note)

> A login account grant (#858, #59) is a per-project grant of the same kind: it names the
> project and the account, records who, when and through which client, and is revocable
> (applied at the next launch). No path creates one without an owner decision.

### #833 (add to Acceptance)

> - With the sandbox on, `torlek doctor` reports per provider whether a credential is bound
>   for sandboxed sessions. A provider signed in only through the owner's own folders reports
>   `missing-prerequisite` for sandboxed sessions, with the credential to set. A Codex ChatGPT
>   plan or OpenCode OAuth login says it is not available to sandboxed sessions until #59.
> - A sandboxed Claude launch that asks for Remote Control on a `setup-token` binding is
>   refused with the reason (the token "can only make model requests"), not started and left
>   to fail.

Also touched, one line each: #685 gains two probes (read a provider credential from inside;
send a placeholder to a non-provider host). [Report 18](../design/greenfield/18-usage-quota-and-accounts.md)'s
"injects the secret as a proxy placeholder as today" now means #832's binding, with login
accounts by project grant; this report does not edit it. #831 is unchanged: provider hosts
stay allowed from the adapter's declaration.

## Sources, limits and what was not checked

- Code and legacy files as cited, read at `26e6e35b`. Issues #59, #830, #829, #831, #832, #833,
  #674 and #856 with their comments, and the #857 and #859 reports, on 2026-11-05.
- Credential files on the research host: key names and file modes only.
- Claude Code: [authentication](https://code.claude.com/docs/en/authentication) (storage,
  precedence, `setup-token`, several accounts through `CLAUDE_CONFIG_DIR`). Codex:
  [authentication](https://learn.chatgpt.com/docs/auth) (`auth.json`, credential stores,
  automatic refresh, access-token login for Enterprise workspaces).
- Nothing was run against a provider and nothing was prototyped. Unproven, for the
  implementing issues to test: whether each harness accepts a placeholder of its token's shape;
  whether each harness honors its runtime's trust variable for an intercepted provider host;
  the access-token lifetimes C would rely on; whether a login mounted alone (without the rest
  of the account's folder) is enough for each harness, and whether its write-back survives a
  bind-mounted file; whether two harnesses refreshing one login at once both stay signed in;
  whether interactive Codex takes an API key from its environment.
