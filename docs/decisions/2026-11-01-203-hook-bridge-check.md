# The daemon proves at its start that the Claude hook bridge runs, and says so in `provider list`

Issue #203. Decided by the worker, following the issue's option.

- **`cockpit-claude-hook --version` prints `cockpit-claude-hook <version>`;** the daemon runs
  it at its start (`Bridge::check`, `crates/cockpit-claude/src/lib.rs`) after finding both
  programs, with a 10 s deadline, and requires this release's exact line. A hook that cannot
  execute (wrong architecture, lost execute bit), answers as another program or as another
  release, or a `cockpit` without its execute bit, leaves Claude Code without its bridge,
  logged with the reason. `cockpit` itself is not run: its verbs ask a daemon not yet ready.
  Claude Code never passes `--version`: it runs the hook with the `cockpit` program.
- **`provider list` gains an `events` column** (`ProviderView::events`, from the provider's
  `Events` capability): `claude\tresume\tevents`. That is where the owner, and the release's
  clean-install check, read whether the daemon found a bridge that runs. Chosen over
  `version --all`, which does not ask the daemon.
- **`tools/release.py verify-install` starts the daemon with a stand-in `claude`** that
  answers only `--version`, then requires `claude\tresume\tevents` from `cockpit provider
  list`, so a bundle whose hook is present but unusable fails the install job.
- **Rejected:** checking the hook's architecture from its file header: running it is the
  proof the issue asks for, and covers a lost execute bit too.
