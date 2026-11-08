# A login account reaches only the projects granted it, bound read-write in the session's home

Status: accepted

Issue #59, parts 3b, 3c and 4a. Passing over a login account the project holds no grant
for, naming it and the missing grant, and refusing a restart kept on it was decided by Theo
(2026-11-08, option C), as was one session at a time per login account (2026-11-08,
choice A). The read-write login mount, the file or a folder holding only it,
comes from #858. The rest are engineering defaults listed in the pull requests.

- **A grant is the owner's, per project.** `torlek account grant PROJECT PROVIDER ACCOUNT`
  records it in `sandbox_login_grants` like a network grant (#674): when and through which
  client, until revoked, one active grant per project, provider and account, rows never
  deleted. No agent request grants one. Removing a project revokes its grants, so a project
  registered later under the same name starts with none.
- **A launch reads the grants when it chooses the account.** The routing that picks the first
  listed account with a credential treats a signed-in login as usable only while the project
  holds a grant for it; a revoke reaches the next launch.
- **The mount follows how the harness rewrites the file.** Claude Code 2.1.319 truncates
  `~/.claude/.credentials.json` in place and opens it with `O_NOFOLLOW`: the login file is
  bound at that path. Codex writes a new `auth.json` beside the link's target and renames it
  over the target, which a bound file refuses: the account's login folder is bound at
  `~/.torlek-login` and `~/.codex/auth.json` links to the login in it. `Provider::login`
  declares which (`LoginRewrite`).
- **The daemon prepares the bind's place, not the agent.** The session's home is the agent's
  to write, so before each launch the daemon replaces a link or other entry at the bind's path
  and refuses the launch when a folder above it is a link: the bind never lands where the
  agent pointed it.
- **An account with both a credential file and a login refuses the launch.** Which one to use
  is the owner's to say.
- **One session at a time uses a login account.** Two holders of one rotating refresh token
  sign each other out when they refresh in turn (#858 report, T5). A launch passes over a
  login another live session (starting, running or stopping) bound, naming that session, and
  refuses a kept one; the transaction that records the launch checks again, so of two
  launches choosing at once the second is refused rather than both taking it.
- **Rejected:** copying the login into the session's home (a refresh would never reach the
  account, and a refresh in one copy leaves the other stale); binding Codex's file
  (its rename fails on a mount point); sessions sharing one login at once (the T5 race).

Evidence: `crates/cockpit-daemon/tests/account_grants.rs`, the login tests in
`crates/cockpit-daemon/tests/sandbox.rs`, `crates/cockpit-sandbox/tests/login_mount.rs`.
