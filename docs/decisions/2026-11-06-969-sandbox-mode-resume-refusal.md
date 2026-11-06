# A conversation is launched again only under the sandbox setting of its session's first launch

Issue #969. Decided by the worker: the smallest behaviour that never widens what a sandboxed
agent reads.

- **Refused before launch, naming the change.** A sandboxed agent's home is the session's own
  folder (#858), an unsandboxed one's is the owner's, and the provider keeps the conversation
  in that home. Resuming it under the other setting could only fail (`No conversation found`),
  so restore, wake, resume and restart refuse it when `agents.sandbox` no longer matches the
  session's first recorded launch (`cockpit_sandbox::first_launch_was_sandboxed`). A restore
  leaves the session `failed` with `restore refused: ...`; an owner operation leaves it as it
  was. Either way the reason says which setting reopens it, or to start a new session.
- **The first launch, not the latest.** Sessions that hit #969 recorded a failed sandboxed
  launch after their unsandboxed one; their conversation is still in the owner's home. Known
  limit: a session whose first agent never named a conversation, restarted under the other
  setting, is refused although its new conversation is where that setting looks. A refusal,
  never a wider sandbox. A named conversation without recorded evidence predates #61: unsandboxed.
- **A restart is refused before its stop**, the deferred one included, so its agent keeps
  running. Sleep is not refused: the conversation is kept, and the wake names the setting.
- **Rejected:** copying the conversation into the sandbox. A transcript written unsandboxed can
  hold anything that agent read with the owner's rights; copying it in would show it to a
  sandboxed agent. Not to be reopened without Theo's decision.
- **Rejected:** showing the owner's provider folders to a sandboxed agent, for the same reason.

Evidence: `crates/cockpit-daemon/tests/sandbox.rs` (`*first_launched*`,
`a_refused_session_resumes_its_conversation_once_the_sandbox_is_off_again`).
