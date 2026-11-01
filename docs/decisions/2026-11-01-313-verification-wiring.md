# Verification wiring: the owner runs checks, the daemon records them, agents claim

Issue #313. Decided by the worker within reports 06, 09 and 11; nothing here needed Theo.

- **A project declares its checks under `[[checks]]` in its `.cockpit/config.toml`**
  (`name`, `command`, `working_directory` default `.`, `environment` default `native`,
  `artifacts`). The owner file refuses them: a check describes a project (report 09).
- **`check-run` is the owner's alone.** It runs the project's command on the host, and the
  checkout's configuration is a file the agent can edit; the owner's request is what grants
  the execution (report 11). An agent's way to a receipt is #353.
- **Only in a checkout the owner trusts** (`projects.trusted`, inherited by its parallel
  clones), never in a review checkout: the command comes from the checkout's own files.
- **Only `native` checks run**, as `/bin/sh -c`, whatever the owner's login shell (a fish
  `$SHELL` would read POSIX commands differently), with the approved commands' environment
  (Git's safe settings included). Any other environment is refused before anything starts
  (#352).
- **The receipt is the daemon's own observation.** Run id `execution-<id>`, the execution
  of the daemon's supervisor; artifacts fingerprinted before the launch; source from the
  daemon's Git; outcome from the exit status (a signal is `failed`, a lost status
  `unknown`, a launch whose confirmation timed out is recorded `unknown` at once, since it may
  still run); output the end of the stdout file, its full size counted. stderr is folded into
  stdout (`exec 2>&1`) so the kept tail reads in the order it was written.
- **The dirty digest is a tree object written from a private index and object directory**
  (`Git::source_state`): `read-tree HEAD`, `add --all`, `write-tree` under a scratch
  `GIT_INDEX_FILE` and `GIT_OBJECT_DIRECTORY` with the repository's objects as alternates.
  The checkout's index and object store are never written; ignored files do not count; the
  digest is `None` when the tree equals `HEAD^{tree}`. Rejected: hashing `git diff` output,
  which misses untracked files and depends on diff settings.
- **A claim needs `change-state` over the session filing it**, as tasks and workflows
  records do; it names the checkout the session works in and is shown `unverified`.
- **The gate is evaluated after the read transaction ends**, in `spawn_blocking`, because
  evaluation re-hashes artifacts in the checkout.
- **A daemon that stops while a check runs records nothing for it** (#351); `cockpit check
  run` waits on the one connection it started the run on, so the wait fails with it.
- **Known gaps:** clean filters (#358), the artifact list outside the definition (#359), an
  agent's project read from its working directory (#360).
