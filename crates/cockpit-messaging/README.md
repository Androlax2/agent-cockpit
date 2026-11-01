# cockpit-messaging

Durable delivery envelopes, receipts, retries and change-feed cursors; producers own what a
message or an entity means.

Reports 04 and 07: one messaging path, shared by teams, Chief and clients, instead of an
inbox per feature. It moves effects and state; it decides nothing about them.

## Outbox

1. A producer calls `enqueue` in the transaction that changes its state, so the effect intent
   commits with that state or not at all. `effect_key` is the one idempotency key of the
   logical effect: enqueuing it again with the same content finds the existing envelope
   (`AlreadyEnqueued`); with other content it is refused (`EffectKeyReused`).
2. A producer's worker takes its own due envelopes with `claim_due` and commits (`submitting`), delivers outside
   any transaction, then records the outcome with `record_delivery`. `deliver_due` is that
   pass over a `Deliverer`.
3. `received` needs the receiver's receipt. A receipt is recorded whenever it arrives, late
   ones included. A failure counts only for the current attempt (fencing).
4. `Failure::NotSent` (nothing reached the receiver) is retried while attempts remain.
   `Failure::Unacknowledged` (a lost acknowledgement, a timeout) is retried with the same key
   only when the receiver deduplicates (`Redelivery::ReceiverDeduplicates`); otherwise, or
   once attempts run out, the delivery is `uncertain`. An uncertain delivery is never sent
   again on its own: `resolve_uncertain` settles it from a lookup's receipt or the owner's
   decision (`uncertain_envelopes` lists them).
5. `reconcile_deliveries` at startup treats every `submitting` envelope as unacknowledged.
6. Only `Coalescing::ReplaceableObservation` envelopes coalesce: a newer one for the same
   destination and slot marks the one still `queued` `superseded`. Decisions, approvals,
   outcomes and ownership transitions use `Coalescing::Never` and are all delivered.

States: `queued → submitting → received | uncertain | failed | expired`, and `superseded`.

## Receipts

`receive` is the receiver's side when Cockpit receives: one durable receipt per receiver and
effect key, written in the transaction that applies the effect. `Received::First` applies it;
`Received::Duplicate` answers with the same receipt and applies nothing.

## Change feed

- A producer calls `publish` with an entity's complete current state, in the transaction of
  the change, and `retract` when it is gone. An unchanged state writes nothing, so an
  identical observation sends no batch. Tombstones are kept, so an entity's revision
  never goes back.
- In-transaction publishing stays the contract for producers that own their writes; derived
  views may be projected by the daemon on commit (sessions and checkouts, #199), retracting
  what `published_ids` lists and its source no longer has.
- Revisions are Turso's global commit revisions (CDC, `cockpit-turso`). `Feed::open` reads the
  snapshot and its revision in one read transaction, or the changes after a cursor; `Feed::next`
  reads the commits since. A batch carries each changed entity once, in its state at the
  batch's cursor; commits that change no entity produce no batch.
- A cursor of another epoch, past the newest revision, or older than the retained history is
  refused `ResnapshotRequired`, never answered with a silent jump.
- `discard_history` keeps the newest change rows and records where the retained history
  starts.

## Dependency edges

- Depends on: `cockpit-turso` (transactions, migrations, change capture) and `serde_json`
  (published states are JSON objects).
- Never on `cockpit-protocol`, a client or a product capability: producers depend on
  messaging, and the daemon converts to wire types (`cockpit-workspace-rules/tests/crate_graph.rs`).
- Used by: the daemon, which serves `subscribe` and reconciles the outbox at startup.

## Tests

- `tests/outbox.rs`: the delivery state machine on a real state database: duplicate
  enqueue, duplicate delivery, lost acknowledgement with and without a deduplicating
  receiver, stale failures, restart, expiry, coalescing.
- `tests/feed.rs`: snapshot, resume, unchanged publishes, tombstones, retention and epochs.
- `tests/published_ids.rs`: the published ids of one kind, tombstones left out.
- The feed over the socket is proven in `cockpit-daemon/tests/change_feed.rs`.
