# Codex remembers the previous conversation within each launch

Issue #408. Decided by the worker to preserve the existing channel's conversation checks.

- **The bridge records conversation handoffs in a private launch-local ledger.**
  Codex's measured `SessionStart` with `source: clear` names only the new thread. The bridge
  can now report the recorded thread's end, then the successor, through the existing CLI.
- **Hooks and failed-turn reports share a file lock.** An old thread's report cannot consume
  the end announcement between those two calls. Once the end succeeds, the predecessor is
  retired even if the successor report fails, preserving the pending handoff across later
  old events. A retry, next prompt or later clear can finish it. Each launcher gets a fresh
  file and removes it on exit.
- **The ledger grants no authority.** The daemon still checks launch identity and conversation
  on both calls. No session SQL, agent-channel fields or provider dependency edges change.
- **Rejected:** a successor claiming the right to replace an unnamed predecessor in the agent
  protocol; the adapter can supply the predecessor without weakening that boundary.
- **Evidence:** [clear process tests](../../crates/cockpit-codex/tests/clear.rs) and the
  [launcher fixture](../../crates/cockpit-codex/tests/session.rs). Live in-session resume/fork
  semantics remain unmeasured; only `clear` initiates a handoff here.
