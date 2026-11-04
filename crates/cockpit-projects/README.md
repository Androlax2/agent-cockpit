# cockpit-projects

Project identity, checkouts (parallel and review clones) with their incarnation, path validation, checkout occupancy and the safe host-Git rules they run under.

A project is a registered Git work tree, named once. Its checkouts are the main work tree,
parallel clones (one branch each) and review clones (one pull request head each), all under
`<parent of the project root>/.parallel/<project>/`. A checkout's id is never reused; its path
is a locator, not its identity (report 04): each row records an incarnation (counted per
path) and a random marker file written into its `.git` directory, and a checkout without that
marker is refused as `Replaced` instead of being acted on.

## Rules

- **Clones.** `git clone --no-local --reference <main> --dissociate`, then origin is set to
  the main checkout's origin. Parallel clones get Cockpit's lefthook runners when the project
  has a lefthook configuration; review clones get no hooks. A failed clone leaves no
  directory and no row.
- **Trust.** `trust` reads the owner's `projects.trusted`: a parallel clone of a trusted
  project inherits its trust, a review clone is never trusted.
- **Occupancy.** `claim` and `release` record one holder per checkout in the state database,
  so occupancy survives a daemon restart. A second holder is refused, naming the first.
- **Storage report.** `clone_storage` observes the literal `target/` folders of registered
  parallel/review clones, including occupied ones, without changing them. Apparent bytes sum
  regular-file lengths per directory entry (hardlinks count again); this is neither allocated
  nor reclaimable space. Missing, incomplete and unavailable results are distinct. Bounds are
  128 clones, 500,000 directory entries and three seconds per clone (30 seconds per report),
  64 directory levels. A clone over its limit is incomplete and names how far the walk got.
  Symbolic links are never followed. Concurrent changes prevent an atomic snapshot; occupancy is the registration snapshot, not permission to prune. See the
  [decision](../../docs/decisions/2026-11-03-406-clone-storage-report.md).
- **Target pruning.** A registered clone's `target/` is pruned only under the owner's policy
  and an immutable confirmed plan, never while the clone has a commit on no remote (the
  sweep's rule). `prune_verdict` tells the storage report what prune would decide now, from
  the report's measurement. See the
  [pruning](../../docs/decisions/2026-11-03-406-target-pruning.md) and
  [verdict](../../docs/decisions/2026-11-04-406-prune-verdict-and-unpushed-guard.md) decisions.
- **Sweep.** Removes clones with no holder, no uncommitted changes, no stash and no commit
  (on a branch or HEAD) missing from the remotes; every kept clone carries its reason. A
  clone is marked `removing` (claims refused) before it is checked and `deleting` once the
  checks pass; only `deleting` skips them. Clones with submodules or a committed nested
  repository are kept, and so is a held clone whose directory is gone. Age and ignored
  files never count. A commit on no remote whose changes are already in `origin/HEAD` (a
  squash-merged pull request whose remote branch was pruned) does not count as unpushed;
  one that changes nothing does. `sweep_clone` sweeps one clone, as the daemon does when
  the session that occupied it ends (#767), and `EndSweep` records what it did, for
  `torlek storage`. See the
  [decision](../../docs/decisions/2026-11-04-767-sweep-on-session-end.md).
- **Host Git.** Every call runs with `SAFE_CONFIG` (hooks off, program-running and
  signature settings reset), every filter, diff and merge driver the configuration names reset, no `GIT_*`
  variable from the daemon's environment and no terminal prompt. `Git::changed_files` reads
  what work in a checkout changed outside the default branch, for the approvals' build-file warning
  (#156). `Git::source_state` reads the head and a digest of the uncommitted changes a
  verification receipt names (#313), writing only to a private index and object directory.
- **Push.** Only the current branch, only to an `origin` with one URL that pushes to one URL,
  both on GitHub after every rewrite, with no local or worktree redirect or `http.*` setting, only as `refs/heads/B:refs/heads/B` without force, so a rewrite is
  refused by Git as a non-fast-forward.

## Dependency edges

- Depends on: `cockpit-turso` (state and migrations) and `tokio` (`spawn_blocking` for Git
  and filesystem work), plus the existing `rustix =1.1.6` pin for descriptor-relative storage
  measurements without following symbolic links.
- Must not depend on `cockpit-sessions`, `cockpit-tasks` or `cockpit-scheduling`:
  checkouts outlive tasks (report 03); `tests/crate_graph.rs` enforces it.
- Used by: the daemon, whose `Routes` serve the `project` and `checkout` verbs to the owner,
  read changed files for approvals and the source a check ran on.

## Tests

- `tests/registration.rs`: registration, names, overlap and path validation on real Git.
- `tests/occupancy.rs`: claim, release, conflicts and replaced checkouts on a real database.
- `tests/sweep.rs`: a clone left `removing` is checked again, not deleted; a squash-merged
  branch is removed, a later or empty commit kept; a main checkout is refused.
- `tests/changed_files.rs`: uncommitted and unpushed changes on real Git.
- `tests/source_state.rs`: a checkout's head and uncommitted-changes digest on real Git, the
  index left as it was.
- `tests/storage.rs` and `src/storage_tests.rs`: registered identity, byte accounting,
  partial results, traversal limits and filesystem replacement races in fixture homes.
- `cockpit-cli/tests/projects.rs`: issue #28's acceptance through the CLI and daemon with a
  fake `git` on `PATH`.
