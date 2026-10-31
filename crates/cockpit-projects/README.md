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
- **Sweep.** Removes clones with no holder, no uncommitted changes, no stash and no commit
  (on a branch or HEAD) missing from the remotes; every kept clone carries its reason. A
  clone is marked `removing` (claims refused) before it is checked and `deleting` once the
  checks pass; only `deleting` skips them. Clones with submodules or a committed nested
  repository are kept, and so is a held clone whose directory is gone. Age and ignored
  files never count.
- **Host Git.** Every call runs with `SAFE_CONFIG` (hooks off, program-running and
  signature settings reset), every filter, diff and merge driver the configuration names reset, no `GIT_*`
  variable from the daemon's environment and no terminal prompt.
- **Push.** Only the current branch, only to an `origin` with one URL that pushes to one URL,
  both on GitHub after every rewrite, with no local or worktree redirect or `http.*` setting, only as `refs/heads/B:refs/heads/B` without force, so a rewrite is
  refused by Git as a non-fast-forward.

## Dependency edges

- Depends on: `cockpit-turso` (state and migrations) and `tokio` (`spawn_blocking` for Git
  and filesystem work).
- Must not depend on `cockpit-sessions`, `cockpit-tasks` or `cockpit-scheduling`:
  checkouts outlive tasks (report 03); `tests/crate_graph.rs` enforces it.
- Used by: the daemon, whose `Routes` serve the `project` and `checkout` verbs to the owner.

## Tests

- `tests/registration.rs`: registration, names, overlap and path validation on real Git.
- `tests/occupancy.rs`: claim, release, conflicts and replaced checkouts on a real database.
- `tests/sweep.rs`: a clone left `removing` is checked again, not deleted.
- `cockpit-cli/tests/projects.rs`: issue #28's acceptance through the CLI and daemon with a
  fake `git` on `PATH`.
