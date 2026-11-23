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
  [decision](../../docs/decisions/2026-11-03-406-target-pruning.md).
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
  files never count. A commit on no remote whose touched paths all read as `origin/HEAD`
  or one of its commits since the merge base left them (a squash-merged pull request whose
  remote branch was pruned) does not count as unpushed; one that changes nothing does. `sweep_clone` sweeps one clone, as the daemon does when
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
  both `https` repository URLs (`cockpit_forge_api::parse_origin`) after every rewrite, on a host
  the caller lists as one the project may push to. The daemon lists the hosts a forge adapter
  declares for Git pushes whose grant carries the owner's push right, none until the owner binds
  one ([decision](../../docs/decisions/2026-11-22-153-push-right-no-seeding.md)). An SSH origin
  is refused naming SSH, and a host without the right is refused naming the command that binds it.
  The checkout must have no local or worktree redirect or `http.*` setting. The branch is resolved
  to one commit id first and the push is `<id>:refs/heads/B` (#155), so a branch moved meanwhile
  changes nothing sent, and no tag or deletion can be expressed. Git authenticates with the
  caller's push credential, handed to a credential helper for the push host alone through Git's
  environment, never an argument, after `SAFE_CONFIG` dropped every other helper. Without force
  Git refuses a rewrite as a non-fast-forward. `Force::Lease` allows one on the caller's own
  branch alone, with the commit this checkout last pushed there to the same URL as the expected
  tip (`checkout_pushes`, written only after a push succeeded); with no such record the force is
  refused, never leased against the remote's current tip (#857 decision 2).
  `Pusher::Owner { expected }` pushes any branch unscanned; with `expected`, only while the
  checkout is on that branch at that full commit id. `Pusher::Agent` pushes only a branch its `AgentBranches` cover (the launch branch,
  or one a push-branch pattern names), never the remote's default branch (`ls-remote --symref`;
  a remote naming none is refused), and only to the repository the main checkout's origin
  (fetch) URL names. A sandboxed agent writes no `.git/config`, so it rewrites neither; in the
  main checkout the two are one, and an agent outside the sandbox can rewrite both. It refuses a push that
  changes what CI runs as `Error::WorkflowChange`, naming the owner's
  `torlek checkout push ID --branch BRANCH --commit COMMIT` (`push_range.rs`). The check compares the
  pushed commit's `.github/workflows` and `.gitlab-ci.yml` entries with those at the remote
  branch's tip and at the remote default branch's tip (read with `ls-remote`, fetched by id
  when missing). It reads no history, because the agent can rewrite the checkout's (replace
  refs, grafts, planted objects), and Git hashes every object it reads again, which must match
  its id. So a branch started before the default branch changed its workflows is held until
  rebased onto it. The tips come from `ls-remote` on the URL checked, run outside the
  checkout so its configuration cannot rewrite it; the push goes to that URL too. The check
  covers the workflow files #857 names, not what they reach elsewhere in the repository
  (scripts, local actions, a GitLab `include: local`): CI runs those like any other code.
  Before that check, every commit the push sends that the remote lacks (`rev-list` from the
  commit without any remote branch's tip the checkout has, replace refs off as `pack-objects`
  has them) must carry an `Agent: <provider> via Torlek <session>` trailer, as Git parses
  trailers, and no signature header; otherwise the push is refused naming each commit and the
  rebase that rewrites them (`attribution.rs`, #694). The session it names is the agent's
  word: the check reads its form, not which session made the commit. A merge of a signed tag
  keeps that tag's signature in its `mergetag` header and passes: the commit itself is
  unsigned. Grafts and a shallow boundary apply here as they do to the pack: a commit they
  hide is not sent either, and the remote refuses the push for lacking it. `agent_trailer`
  writes the trailer a session's launch hands its agent in `COMMIT_TRAILER_VARIABLE`
  (`TORLEK_COMMIT_TRAILER`).
- **Push branches.** `project_push_branches` holds the branch patterns the owner adds per
  project (`add_push_branch`, `remove_push_branch`, `push_branches`), none by default
  (#155, 2026-11-09 option B). `BranchPattern` refuses a pattern that names no branch or
  reaches beyond branches (`refs/`, `..`, an empty segment); `*` matches within one segment,
  a `**` segment any number of them.

## Dependency edges

- Depends on: `cockpit-turso` (state and migrations), `cockpit-forge-api` (the origin URL a
  push may go to, with no forge named) and `tokio` (`spawn_blocking` for Git and filesystem
  work), plus the existing `rustix =1.1.6` pin for descriptor-relative storage
  measurements without following symbolic links.
- Must not depend on `cockpit-sessions`, `cockpit-tasks` or `cockpit-scheduling`:
  checkouts outlive tasks (report 03); `tests/crate_graph.rs` enforces it.
- Used by: the daemon, whose `Routes` serve the `project` and `checkout` verbs to the owner,
  read changed files for approvals and the source a check ran on.

## Tests

- `tests/registration.rs`: registration, names, overlap and path validation on real Git.
- `tests/occupancy.rs`: claim, release, conflicts and replaced checkouts on a real database.
- `tests/sweep.rs`: a clone left `removing` is checked again, not deleted; a squash-merged
  branch is removed even after the default branch changed its file, a later, empty or partly
  undoing commit kept; a main checkout is refused.
- `tests/changed_files.rs`: uncommitted and unpushed changes on real Git.
- `tests/push_branches.rs`: push-branch patterns added once each, listed in name order,
  removed, and refused for an unregistered project.
- `tests/push.rs`: a push only to a host the project may push to, SSH and credential origins
  refused, the resolved commit sent though the branch moves, the push token offered to the push
  host alone, every lease rule, the agent's branch rules (launch branch, patterns, never the
  default branch, the project's repository alone), the owner's push pinned to a commit, and
  the workflow check (a rebase onto the default branch, replace refs and grafts that would
  hide a change). It runs
  real Git, pushing to a local bare repository through a wrapper that rewrites the forge URL
  for the remote calls alone.
- `tests/source_state.rs`: a checkout's head and uncommitted-changes digest on real Git, the
  index left as it was.
- `tests/storage.rs` and `src/storage_tests.rs`: registered identity, byte accounting,
  partial results, traversal limits and filesystem replacement races in fixture homes.
- `cockpit-cli/tests/projects.rs`: issue #28's acceptance through the CLI and daemon with a
  fake `git` on `PATH`.
