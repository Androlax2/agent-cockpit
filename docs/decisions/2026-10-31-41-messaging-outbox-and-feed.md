# Messaging publishes entities explicitly, never resends an unacknowledged effect blindly, and streams the feed from change capture

Issue #41. Decided by the worker, reading report 04 and report 07 as written.

- **Producers publish entities; the feed does not infer them from change rows.** A producer
  calls `publish` or `retract` in its own write transaction, which writes
  `messaging_entities`; the feed reads the change rows of that table only. Change capture
  then gives the commit revision and the order, and no capability's tables leak to clients.
  An unchanged state writes nothing, so it reaches no subscriber.
- **Retracted entities stay as tombstones.** Their revision keeps growing, so a later
  publish of the same id never goes back to a revision a client already applied.
- **A lost acknowledgement is retried only when the receiver deduplicates.** Each envelope
  says `receiver-deduplicates` or `never`; an unacknowledged `never` is `uncertain` at once,
  and every delivery left `submitting` by a stopped daemon is unacknowledged. Retries that
  run out leave `uncertain`, never `failed`. Only the owner settles an uncertain delivery,
  once. Failures are fenced on the attempt number, so a late failure changes nothing; a late
  receipt always counts.
- **Only replaceable observations coalesce.** An envelope naming a `replaceable` slot
  replaces the one still queued in the same destination and slot, which becomes
  `superseded`; anything submitting or later, and every decision, is never replaced.
- **`cockpit-turso` gained a commit watch and `discard_changes`.** Subscribers wake on
  commit without polling. Discarding keeps the newest commit, so revisions never repeat,
  and messaging records `retained_after` so an older cursor gets `resnapshot-required`
  instead of a silent gap. The daemon keeps 100,000 rows, hourly.
- **The feed is the owner's only.** `change-feed` is offered on `daemon.sock` alone. One
  feed per connection; a stopped feed closes its connection so the client resubscribes.
- **Messaging depends on the database only** (`cockpit-workspace-rules` allow-list):
  producers depend on it, and the daemon converts its types to wire types.
- **Rejected:** polling the change table (latency against wakeups for nothing); decoding
  change rows' before and after images into entities (couples the feed to every table's
  layout); a separate sequence counter (a second source of order that can disagree with the
  commit revision).
