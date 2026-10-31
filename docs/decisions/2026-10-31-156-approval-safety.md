# Approval safety: what the daemon warns of, and the grace it enforces

Issue #156. The grace and the split with the TUI decided by Theo in #166 (option A plus C);
the rest by the worker, for the reasons below.

- **Pane behaviours wait for the TUI (#35).** The deliberate approve key, the grace after the
  pane appears or takes focus, focus never jumping while the owner types, and rendering the
  warnings, the fold and "waiting on you" are acceptance items of #35. This issue ships the
  data every client renders: warning kinds on `ShownText`, `folded_command`,
  `timeout_without_foreground`, `changed_build_files`.
- **The daemon refuses an approval within `APPROVAL_GRACE_SECONDS` (2) of the revision
  appearing** (ask or edit, `approvals_runs.revised_at`, migration 2), as `ErrorCode::Refused`
  with an owner unblock. It holds for every client, the phone and the CLI included. Times are
  whole seconds, so the grace lasts between one and two seconds; `approve_run` takes the clock
  as an argument, which the crate tests set. Declining has no grace: it is the safe answer.
- **"Changed by the session" is "changed in the checkout and on no remote"**
  (`Git::changed_files`: uncommitted, untracked included, plus files of commits on no remote
  branch). The daemon cannot tell which session wrote a file; a checkout belongs to one
  session at a time, and a file pushed already was reviewable elsewhere. A repository with no
  remote counts every commit, so its warnings are many rather than missing.
- **Changed build files are read when the owner looks** (`approval show`, the queue, an
  edit), not when the agent asks, so a file changed after the ask still shows. Only asking
  requests in a known project are read; otherwise the field is `None` and the CLI says it
  could not tell.
- **Which files a command runs is read from its words, generously** (`build_files_run`): a
  tool's build files (`make`, `cargo`, package managers, `gh`/`act`, `git` and hook
  managers) and any changed file the command names. A warning too many is read and
  dismissed; one missing hides what runs.
- **`ask run --no-wait`** files the request and exits 124 at once with the `--wait ID` that
  collects the answer; the request was already asynchronous on the channel, only the CLI
  blocked. Run execution is untouched (#163 owns it).
- **Rejected:** storing the build-file warning with the request (misses later edits of the
  files); a new `too-soon` error code (`refused` already means "a safety rule refused it,
  the message names it"); millisecond request times (a schema change for a backstop the TUI
  refines anyway).
