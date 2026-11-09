# The greenfield test shards are cut by measured time, and each still builds for itself

Status: accepted

Issue #1261. Theo asked for the CI wall time to be cut, measuring before and after; the worker
chose the layout from the runs cited in the pull request.

- **`greenfield-test` runs four shards of whole test binaries, balanced by seconds.**
  `tools/ci-shards.py plan` reads `cargo nextest list` and assigns the binaries, longest first,
  to the lightest shard, from `tools/ci-test-weights.json`. Every shard cuts the same list the
  same way, so every binary runs in exactly one shard; a new binary weighs a default second and
  a stale entry is ignored, so a stale file only unbalances the shards and never drops a test.
  The hash partition it replaces could not balance by time: one shard held the slow binaries.
- **The weights come from the shards themselves.** The `ci` nextest profile
  (`.config/nextest.toml`) writes a JUnit report; each shard uploads it as `junit-shard-N`, and
  `tools/ci-shards.py weights` turns the reports into the weights file.
- **Rejected, measured: building once with `cargo nextest archive` and unpacking it in the
  shards, the sandbox job and WSL2.** The test binaries statically link the database engine, so
  the archive is 2.0 to 2.6 GB. In run 42669613258 the build job took 595 s with a 182 s upload,
  and each shard's download took 45 to 106 s, against a baseline whose whole pull request run
  took 651 s (run 42628621421). Transfer costs more than the builds it saves, as
  `2026-11-06-997-sharded-greenfield-tests.md` estimated. Sharing it with `greenfield-wsl2`
  would also pay the transfer and need the build's absolute path.
- **The required check is still `greenfield`;** no check is dropped or weakened and the ruleset
  is unchanged.
