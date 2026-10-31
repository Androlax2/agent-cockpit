# cockpit-turso

The state database's connections, transactions, migrations, backup and change capture.

It owns the state database's infrastructure: opening the one Turso file (single opener, change
capture on), write and read transactions, the ordered migration runner, online backup and
offline restore. It owns no feature queries: each capability owns its own SQL and migrations,
receives a transaction from its caller and runs SQL through `cockpit_turso::sql` (report 02,
report 04).

## Dependency edges

- Depends on: `turso` (SQLite-compatible mode, no MVCC or experimental flags) and `tokio`
  (`sync` only, for the single writer connection).
- Depends on no `cockpit-*` crate.
- Every capability that stores state depends on it; no other workspace member depends on
  `turso` directly, so every connection carries change capture and the single-writer rule.

`tests/crate_graph.rs` enforces both edge rules.
