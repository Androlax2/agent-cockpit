# A coordinator ends the workers it started once the task list says their work is done

Issue #713 (v0.1.0-alpha.3). Decided by the worker, within the issue's Expected and the
coordinator kit (`2026-11-01-410-coordinator-kit.md`); the open questions the issue left are
answered below.

- **A fourth coordinator grant, `end-sessions`.** `Permission::EndSessions` joins
  `Permission::COORDINATOR`: given with `torlek session grant SESSION end-sessions`, revoked
  with `revoke`, and revoked with the others when the holder ends. It takes no terms: an end
  is bounded by the sessions the holder started and finished, so no cap per hour and no owner
  confirmation. `--per-hour` and `--without-confirmation` on it are refused as on the other
  non-start grants. Evaluated over the target session, like `message-sessions`, so a narrower
  grant can come later without a wire change.
- **"Sessions it started" is the `started_by` link of #710**, never a lookup in the audit:
  the session row says which coordinator asked for it, whether the owner confirmed the start
  or not. A session the owner started has none and is refused as "did not start session".
- **"Done" is the finished label of #157 (decided in #201, #226, #269, #312).** The task list,
  never the agent's own state or outcome, says the work is done: the session is doing no item
  and finished one since its latest working launch (`torlek todo done`). The daemon knows
  nothing of pull requests, so a merged PR is not a condition; the owner's `session end
  --finished` uses the same rule, and the coordinator's verb replaces it for its own workers
  only. **Rejected:** the agent's `--outcome completed`, because an agent's word about its own
  work is a claim (report 11); the merged PR, because nothing in the daemon records it.
- **The verb is `torlek agent end SESSION`** (`agent-end` on the agent socket), beside `agent
  start`. It runs in the stop lane like the owner's end (#217), never in control (#32). The
  session's reason reads `ended by session <coordinator>`, not `ended by the owner`
  (`cockpit_sessions::Ender`), so the card tells the two apart. The grant, the `started_by`
  link and the finished label are decided under the worker's lock, right before its agent is
  stopped (`Sessions::end_by_when`): a grant revoked or an item taken in between refuses the
  end instead of being missed. **Known limit:** the stopping state keeps no ender, so an end a
  daemon restart interrupts is finished on restore with `ended by the owner`, as every
  interrupted end was before #713; the `ended` audit row stays the record of who asked.
- **Every end is audited** as `ended` (`AuditedAction::Ended`): actor the coordinator, target
  the worker, detail `item <id>` naming the item it finished last. A refused end is not
  audited. Migration 4 of `cockpit-authorization` rebuilds the audit table for the new action,
  as #657 did.
- **Everything else stays owner-only.** `session end` of any other session, `end --finished`,
  `restart`, `sleep`, `wake` and `resume` keep answering "the owner's alone" on the agent
  socket, `end-sessions` or not; `routes.rs` never sends them to the coordinator kit.

Evidence: `crates/cockpit-daemon/tests/coordinator_ends_workers.rs`,
`crates/cockpit-authorization/tests/coordinator.rs`, `crates/cockpit-protocol/tests/coordination.rs`.
