# The clone sweep asks origin for its branch heads before keeping a clone as unpushed

Issue #882. Decided by the worker: the issue's acceptance allowed either sweeping the clone or
naming the stale ref; asking origin sweeps it without weakening the keep rule.

- **The clone's own refs can be stale.** A worker that pushes by URL, or rebases and
  force-pushes, leaves `origin/<branch>` at an older commit, and a worker that ends after
  arming auto-merge never fetches the squash merge. Both #767 rules then saw unpushed commits,
  and every such clone was kept for good.
- **Origin has the last word, and only reads.** When the clone's refs say a tip is unpushed,
  the sweep runs `git ls-remote --heads origin` and counts every commit a branch head origin
  reported contains as pushed (`--ignore-missing`: a head the clone lacks the object of hides
  nothing). The #767 squash-merge rule then applies to what is left. Nothing is fetched or
  written, so `torlek checkout sweep --dry-run` stays read-only.
- **Asked only of a trusted origin, with a deadline.** Origin is asked only when it has one URL
  (after `insteadOf`) that is a GitHub repository URL or the project's main checkout (a project
  without a remote), and the clone's own configuration has none of the push's redirect settings:
  whoever works in the clone writes that configuration. The call is killed after 20 s; ssh runs
  in batch mode.
- **A failed question keeps the clone, and says why.** Origin unreachable, refusing or silent:
  the clone is kept as `unreadable: its remote-tracking refs lack some of its commits, and
  origin could not be asked whether it has them: <error>`. A commit no branch of origin
  contains is kept as unpushed, as before.
- **The target-prune guard does not ask.** It keeps its local-only `has_unpushed_commits`,
  which can only keep more, and stays off the network.
- **Rejected:** fetching before the sweep (writes refs and objects into a clone the owner may
  keep, and a read-only dry run could not do it); GitHub's pull-request API (the #406 pruning
  decision ruled a merged pull request out as proof; a branch head on origin is Git's own);
  asking any origin URL (a clone's configuration could point the daemon anywhere).

Evidence: `crates/cockpit-projects/tests/sweep.rs` (a squash-merged branch with a stale
tracking ref removed; a later commit kept; a remote that is neither GitHub nor the main checkout
not asked; an origin that cannot be asked kept with the reason), the timeout unit test in
`crates/cockpit-projects/src/git.rs`, and `crates/cockpit-cli/tests/projects.rs` (an unpushed
clone with a GitHub origin still kept, through the fake GitHub).
