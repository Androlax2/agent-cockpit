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
  `greenfield-lint` and `greenfield-test` and runs unless the run was cancelled. It fails
  unless both passed, and passes when `changes` selected no greenfield job. Without
  `!cancelled()` a failed shard would skip it, and a skipped required check counts as
  passed. The ruleset is unchanged.
- **The shards keep the cache entry the single job saved** (`shared-key: greenfield` is
  that job's key), and only shard 1 saves it on `main`: the three build the same target.
  `greenfield-lint` gets its own entry: clippy and rustdoc build other artifacts.
- **Shard 1 runs the second-user sandbox step, shard 2 the live probe**, so neither adds
  to the same shard.
- **The CLI and TUI end-to-end harnesses build `--workspace --bins --examples`.** With
  `-p cockpit-daemon` Cargo resolved other features than the gate's
  `cargo nextest run --workspace` and compiled the daemon and its workspace crates a second
  time, inside the test run, while every CLI test that starts the daemon waited on that
  build (about 40 s each locally). Examples turn on the dev-dependencies, as the test build
  does, so the harnesses now reuse the gate's own programs.
- **Rejected:** a `cargo nextest archive` build job feeding the shards (above); a fourth
  shard (each one repeats the build and setup for about 35 s less test time); renaming the
  required check (an owner change to the ruleset for no gain).
