# `torlek version --all` asks the running daemon

Issue #64, report 13 ("Visible versions"). Decided 2026-11-06 when the audit of #64 found
this the one scope item no other issue owned.

- **The daemon answers, the CLI does not read the database.** Turso allows one opener per
  file, so `version --all` sends the owner-only `version-report` request
  (`cockpit-daemon/src/versions.rs`). A session that asks gets the agent socket's
  `owner_only` refusal, and the CLI says the report is the owner's instead.
- **Every launch records the release and instruction manifest it ran with**, in
  `session_launch_bundles` (sessions migration 13), a table beside `session_launches` like
  `session_exits`. A launch with no row started before this and shows "bundle not recorded";
  nothing is guessed for it. "Helper version" is the release: helpers install as one bundle
  with the daemon, so the manifest's per-file hashes add nothing a reader can act on.
- **Only sessions whose agent is launching, running or stopping are listed.** An asleep,
  failed or ended session launches again with the bundle the daemon then holds, so the
  previous one it last used no longer matters. Rejected: listing every session, which grows
  without bound and shows thousands of ended rows.
- **Schema compatibility is the migration ledger against the migrations the daemon's binary
  knows**, per capability (`applied` and `known`), not an assumption that startup succeeded.
  A capability in the ledger that the binary does not know is listed with `known` 0. The
  daemon already refuses a newer schema at start, so `INCOMPATIBLE` only shows if that
  refusal is ever bypassed, and then `version --all` exits 1. Rejected: reading the ledger from the CLI, for the one-opener
  reason above.
- **The recorded bundle is what the daemon itself carries, not a probe of the installed
  files.** A session started by a daemon that an in-place install later outdated still names
  the daemon's release, which is the fact that matters: what that agent was launched with.
  Rejected: reading the installed manifest at each launch, which would name files the agent
  may not have read.
- **No daemon is a normal state; a daemon that predates `version-report` still names its
  release.** Only a missing or refusing socket counts as no daemon; any other connect failure
  is a fault and exits 1. An older daemon answers `unknown-command`, and the instruction catalog every daemon since
  #49 serves names its release and manifest. The CLI says to stop and start the daemon when
  its release is not the installed one: that is report 13's "installed, but not running".
- **Existing `--all` lines are unchanged**; the daemon's lines follow the bundle manifest.
