# A placement is delivered through the outbox and charged as a held admission reservation

Issues #385 and #386. Decided by the implementing session, closing the two leftovers of
docs/decisions/2026-11-01-43-scheduling-placements.md ("placing does not deliver", "builds
do not see placements").

- **The pass enqueues the delivery in its own write transaction.** `place` writes the
  placement and a `cockpit-messaging` envelope (producer `scheduling`, key `placement-<id>`,
  destination the session) together, so a placement never exists without its delivery and a
  crash between the two cannot happen. The daemon runs `deliver_due` after each pass.
- **What is typed is one line pointing at durable records.** An item's line names
  `cockpit todo show <item>` and `cockpit todo done <item> --summary`; a step's names
  `cockpit workflow packet <run>`. The brief is never the work itself: Claude Code takes one
  line per submit, and the record stays the source of truth.
- **Only a session whose provider can submit gets work.** `Capability::Submit` is an
  eligibility check with its own explanation (`its agent cannot be sent work`), so a
  provider that cannot type is explained, not placed and then failed.
- **Typing is never repeated after an unconfirmed send** (`Redelivery::Never`): a second copy
  of a brief starts the work twice. A send that never reached tmux is retried on the next pass,
  at most 3 attempts; a failed or expired delivery ends the placement as `delivery-failed` and
  hands the work back (item to `todo` and the queue, step to ready). An unconfirmed send stays
  `uncertain` with its placement active; resolving it is #414.
- **A placed turn holds an admission reservation in the new `held` state.** It is charged in
  `in_use` like a running one, never expires and no sweep releases it, because the turn runs
  inside the session's long-lived execution, which admission cannot observe ending. The
  placement records its reservation and releases it when it ends, in the same transaction.
  The pass then counts room as budget less admission's in-use: placements from before this
  change (no reservation) are still added on top, so nothing is charged twice or not at all.
- **Rejected:** a `running` reservation bound to the session's execution, because one execution
  holds one reservation and a session runs many turns; per-turn expiry, because a long turn
  would lose its charge while still running; delivering outside the pass transaction, because a
  crash would leave a placement its agent never hears of.
