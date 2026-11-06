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

## Revisited 2026-10-31 (#185)

Decided by the worker: the daemon finds `cockpit-claude-hook` only in its own directory
(`crates/cockpit-daemon/src/main.rs`, `claude_code`), so the bundle puts it there. The Claude
hook bridge ships in `bin/` beside the daemon and is the manifest's first helper. The check
above builds on this layout.

- **`bin/cockpit-claude-hook`, built with `-p cockpit-claude`.** `tools/release.py` lists it in
  `HELPERS`, which `BINARIES` includes, so `package` refuses a build without it and
  `verify-install` fails an install that lacks it or a manifest that does not list it.
  `tools/tests/test_release.py` checks that `release.yml` builds and gathers every entry of
  `BINARIES` and that the daemon's lookup names each helper.
- **`helpers` maps a helper's name to its version**, the bundle's own, since helpers build from
  the same revision. Report 13 wants each session to record the helper version it started with;
  a name-to-version map is the smallest record that serves it, and `verify-install` refuses
  any other content. Schema stays 1: the field existed and only its content changes from `{}`.
- **Rejected:** a separate `libexec/` directory, because the daemon would then need a second
  lookup rule; and a smoke run of the hook in `verify-install`, because it needs a Claude
  payload and a `cockpit` socket, which the bridge tests in `cockpit-claude` already cover.
  The `--version` check above is the smaller proof that replaced this rejection.
