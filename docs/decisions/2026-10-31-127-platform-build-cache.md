# The macOS job restores a build cache saved on main; the WSL2 job stays uncached

Issue #127. Decided by Theo for the cache action (he pre-approved one pinned
`Swatinem/rust-cache` on the macOS job) and by the worker for the rest, from the job
durations of the five green `main` runs before this change (41209922817 to 41215764505).

| Job | Total | Not cacheable | Build and tests |
| --- | --- | --- | --- |
| `greenfield-macos` | 112–214 s | toolchain and installer, about 15 s | 97–193 s |
| `greenfield-wsl2` | 169–228 s | `setup-wsl` 66–86 s, checkout 8–11 s | 84–120 s |

- **`Swatinem/rust-cache`, pinned to v2.9.3's commit, on `greenfield-macos`.** It caches
  the registry and the dependencies' build output, not the workspace crates. Its key adds a
  hash of the manifests and `Cargo.lock` to the toolchain and job; a lockfile change misses
  the key, restores the newest entry for the same toolchain and job, and rebuilds only what
  changed.
- **Only `main` saves (`save-if`).** Pull requests restore from the base branch. `main` saves
  a new entry only when the key changes (an exact hit skips the save), which is when the
  cached dependencies change; a `main` run cancelled by the next merge saves nothing and the
  next completed run does. Saving per pull request would fill the repository's 10 GB cache
  quota with entries no other branch can read.
- **`cache-bin: false`.** `~/.cargo/bin` holds the cargo-nextest that `install-action` just
  installed against a reviewed hash; restoring a cached copy over it would make the cache, not
  the pin, decide which binary runs.
- **WSL2 stays uncached.** `actions/cache` runs on the Windows host and cannot see the
  distribution's file system: a cache means tarring the registry and `target/` across DrvFs
  in both directions on every run, plus a second new action. The build it could save is
  84–120 s, a little over half the job, and the copy through DrvFs would eat into that; the
  largest fixed cost, `setup-wsl`, is not cacheable at all.
- **`rust-toolchain.toml`'s clippy and rustfmt stay.** Both platform jobs download them on
  the first cargo call for nothing (10–30 s, inside the table's last column), but overriding
  the file would state the toolchain version twice; the issue ties that change to the WSL
  cache, which that job does not get.
- **Rejected:** running the platform jobs only on `main` and on `crates/cockpit-daemon/**`
  changes. A change to any crate the daemon depends on would then reach `main` untested on
  macOS and WSL2, which the M0 gate (#27) exists to prevent.
- **Open (`validate`):** the macOS saving is measured on the second `main` run after this
  merges, the first one that can restore a saved cache; the result is recorded on #127.

Regression test: `tools/tests/test_ci_bootstrap.py`
(`test_macos_job_restores_the_pinned_rust_cache_saved_only_on_main`).
