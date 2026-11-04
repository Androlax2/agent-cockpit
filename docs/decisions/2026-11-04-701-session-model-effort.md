# A session's model and effort are chosen at its start, in the provider's own words

Issue #701. Decided by the worker, as the smallest choice consistent with the existing
`LaunchRequest::effort` contract; none of it touches security, so the owner was not asked.

- **Vocabulary: the provider's own words.** `--model` and `--effort` pass through as typed
  (`opus`, `claude-opus-5-5`, `xhigh`), as `LaunchRequest::effort` already did. A Torlek-level
  vocabulary would need a mapping per provider and per harness release, and would hide
  models a harness adds before Torlek knows them.
- **Validation: the adapter, before anything is recorded or asked.** `Sessions::start` and
  the coordinator's `agent-start` build a trial `Provider::launch`, which only builds words,
  before the session row or the approval request exists: a refused value leaves nothing to
  clean up and never reaches the owner's queue. The Claude adapter checks effort against
  `EFFORT_LEVELS` and a model only for its shape (one word, not a flag): Claude Code takes
  any alias or model id and itself refuses one the account cannot use. Approvals check only
  that each is one word, as they do for `--agent`.
- **Providers without the capability: refused.** A model needs the new `Capability::Model`,
  as effort needs `Capability::Effort`; Codex and OpenCode report neither, so a start that
  chooses one is refused naming the capability. Starting without it would run a session on
  a model the owner did not ask for, unnoticed.
- **Lifetime: recorded on the session, fixed at start.** `sessions.model` and
  `sessions.effort` (migration 10) are passed by every launch: restart, wake, resume and
  restore. A `/clear` keeps the running process and so its flags. Changing them on a running
  session is not offered; end it and start another. A stored value a later adapter refuses
  fails that relaunch with the adapter's reason, as any refused launch does, rather than
  silently running the harness's default.
- **Defaults: none in Torlek.** Without the flags nothing is passed and the harness's own
  settings apply (`~/.claude/settings.json`).
- **Coordinator starts: the owner reads the choice.** `approvals_runs.start_model` and
  `start_effort` (migration 8) keep it; the request's command, which the approval digest
  covers, shows `--model` and `--effort`.
- **Deferred:** bounding a coordinator's choice by its `start-sessions` grant (#412, #158), and
  showing model and effort in `session show`, `agent sessions` and the TUI. Both are follow-ups.
- **Rejected:** a `Provider` method to check choices, since the trial launch already is the
  adapter's own check; starting a provider without the capability and saying so.

Evidence: [Claude launch](../../crates/cockpit-claude/tests/launch.rs),
[contract](../../crates/cockpit-provider-api/tests/contract.rs),
[daemon socket](../../crates/cockpit-daemon/tests/sessions.rs),
[coordinator start](../../crates/cockpit-daemon/tests/coordinator_kit.rs),
[approvals](../../crates/cockpit-approvals/tests/starts.rs) and
[CLI](../../crates/cockpit-cli/tests/sessions.rs).
