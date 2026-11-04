# The daemon sweeps a clone when the session that occupied it ends; a squash-merged branch is not unpushed

Issue #767. Decided by Theo on 2026-11-04 (the unpushed rule) and by the worker (the rest,
reasons below).

- **Sweep at session end, default on.** The daemon looks, at each session tick (30 s), for
  clones nobody occupies whose sessions all ended, each one's agent observed exited, and
  sweeps each with the clone sweep's own rules (`cockpit_projects::sweep_clone`). A main
  checkout is never swept. Owner setting `storage.sweep_on_session_end = false` turns it
  off; `torlek checkout sweep` stays for manual use. A polling pass, not a hook in each end
  path, so every way a session ends (owner end, agent exit, asleep expiry) and a daemon
  restart between the end and the sweep are covered without a second trigger. On the first
  start with this change, clones of sessions that ended before are swept too, under the
  same rules.
- **One record per clone incarnation and session end** (`project_end_sweeps`, keyed by the
  number of the session's latest launch): removed,
  or kept with the reason, and what became of a kept clone's `target/`. A clone is swept
  once per end, a failed sweep is recorded rather than retried every pass (a database
  failure records nothing, so a later pass retries), and a resumed session that ends again
  sweeps again. A `target/` prune deferred because another session runs is recorded as
  `not pruned now`; the periodic automatic prune or `torlek storage prune` can still take it. `torlek storage` lists the latest 20.
- **A clone with unpushed commits keeps everything, `target/` included** (Theo, option A).
  The #758 rule wins over the issue's first acceptance ("prune only `target/`"). Only a
  clone kept for uncommitted changes alone goes through the existing automatic `target/`
  prune, which keeps it unless the owner's prune policy allows it without confirmation and
  no other session runs; the record says which.
- **A squash-merged branch is not unpushed.** A tip with commits on no remote-tracking
  branch counts as pushed when every path those commits touched reads the same at the tip
  and in `origin/HEAD` (`git log --name-only` against `git diff --name-only`): every change
  it makes is already there, only commit messages and intermediate states are lost. A tip
  that changes nothing since its merge base (an empty commit) stays unpushed, and so does
  one whose touched file the default branch changed again, one that undid part of its own
  merged change afterwards, and one whose clone never fetched the merge. The target-prune
  guard uses the same rule (`sweep::has_unpushed_commits`). Git only reads, so
  `torlek storage` writes nothing into a clone.
- **Ending a session no longer leaves its clean clone for a resume.** Resuming an ended
  session whose clone was removed is refused by the claim; its work was on the remote.
- **Rejected:** asking GitHub whether the pull request merged (the pruning decision ruled
  it out as proof); recording the remote branch's last commit at push time (workers push
  with their own Git, which the daemon does not see); `git merge-tree --write-tree
  origin/HEAD <tip>` giving `origin/HEAD`'s tree (it counts a later commit that reverts part
  of the merged change as merged, so that commit would be lost; it writes objects into the
  clone during a read-only report; it needs Git 2.38). The path rule keeps a clone whose
  merged file the default branch changed again, which `merge-tree` would remove: kept on
  doubt.

Evidence: `crates/cockpit-daemon/tests/sessions/end_sweep.rs` (clean clone removed, unpushed
clone kept with `target/`, uncommitted clone kept with its `target/` outcome, setting off),
`crates/cockpit-projects/tests/sweep.rs` (squash-merged branch removed, later or empty commit
kept, a partial undo after the merge kept, a file the default branch changed again kept, main
checkout refused), `crates/cockpit-cli/tests/storage.rs`
(the report's session-end section), `crates/cockpit-config/tests/storage_settings.rs`.
