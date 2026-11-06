# The greenfield tests run in three hash-partitioned shards behind one required check

Issue #997. Theo chose sharding plus test speed-ups (option B) on 2026-11-06; the worker chose
the job layout, the shard count and the cache keys from the measurements on the issue and the
pull request.

- **`greenfield-test` runs `make crates-test TEST_PARTITION=hash:N/3` in three matrix shards.**
  nextest's hash partition puts every test in exactly one shard, by its name, so a new test
  does not move the others. On 2026-11-06 the shards held 1145, 1068 and 1102 of the 3315
  tests (`cargo nextest list --partition`), and their union is the full list. `fail-fast: false`
  lets every shard report. Each shard builds the whole workspace: about 3 minutes of build
  per shard is cheaper in wall time than a build job followed by `cargo nextest archive`
  uploads and downloads, which add a job's setup and the archive's transfer to the path.
- **`greenfield-lint` runs `make greenfield-lint`** (fmt, clippy, doc, deny, typos) beside
  the shards. `make greenfield-ci` still runs the whole gate in one go, for the release
  workflow and humans.
- **The required check stays `greenfield`**, now a job that needs `changes`,
  `greenfield-lint`, `greenfield-test` and `greenfield-sandbox` and runs unless the run was
  cancelled. It fails unless all three passed, and passes when `changes` selected no greenfield job. Without
  `!cancelled()` a failed shard would skip it, and a skipped required check counts as
  passed. The ruleset is unchanged.
- **The shards keep the cache entry the single job saved** (`shared-key: greenfield` is
  that job's key), and only shard 1 saves it on `main`: the shards and `greenfield-sandbox`
  build the same target.
  `greenfield-lint` gets its own entry: clippy and rustdoc build other artifacts.
- **`greenfield-sandbox` runs the second-user sandbox step and the live probe beside the
  shards**, on the shards' build (`cargo test --no-run --workspace`, the same cache entry,
  restored only). On a shard, the first pull request run (42264593818) put them on the
  critical path: shard 1 took 589 s, 120 s of it this step, against 362 and 401 s for the
  others.
- **The CLI and TUI end-to-end harnesses build their programs with `--workspace` and the
  `cockpit-e2e-daemon` example.** With `-p cockpit-daemon` Cargo resolved other features than
  the gate's `cargo nextest run --workspace` and compiled the daemon and its workspace crates
  a second time, inside the test run, while every CLI test that starts the daemon waited on
  that build (about 40 s each locally). An example turns on the dev-dependencies, as the test
  build does, so the harnesses now reuse the gate's own programs. They still name only the
  targets they run: `--bins` would relink the `torlek` a focused `-p cockpit-cli` run tests.
- **The shards' cache entry still holds the old job's clippy and rustdoc output** until the
  key next changes (a `Cargo.lock` or toolchain change); restoring it costs seconds, and
  rewriting the entry on purpose would need a second key.
- **Rejected:** a `cargo nextest archive` build job feeding the shards (above); a fourth
  test shard (each one repeats the build and setup for about 40 s less test time); renaming the
  required check (an owner change to the ruleset for no gain).
