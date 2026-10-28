# Protocol and persistence changes

- Deserializing a request does not authorize it. Preserve the daemon's authenticated
  sender checks and the separate host approval path.
- Keep bounded message reads, deadlines, size limits, unknown-field rejection and
  validation at their existing boundaries when moving code.
- Task, session and runtime identities are distinct. Late results must not mutate a
  replacement session or revive revoked authority.
- Team and Chief wire types reference each other; treat their shared contract as one
  extraction boundary. Do not create a cyclic pair of crates.
- Their stores currently share SQLite access. A protocol extraction must not split
  transactions, change schema ownership or grant new access to the database.
- Preserve malformed-input, stale-identity and CLI/daemon roundtrip tests. Add a
  regression test for each new dependency or architectural rule.

The source map and current coupling are in [Architecture](../../docs/ARCHITECTURE.md).
