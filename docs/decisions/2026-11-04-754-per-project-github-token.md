# A project's sessions get its own GitHub token as `GH_TOKEN`; with one, the owner's `gh` login is hidden from them

Issue #754. Decided by Theo (the issue's expectations, and that workers may use the owner's
login until each project has a token) and by the worker (the rest, reasons below).

- **The token lives in a file of the owner's configuration directory:**
  `<config dir>/github-tokens/<project>` (`~/.config/torlek/github-tokens/agent-cockpit` in
  the default profile), one fine-grained token per project, mode `0600` or `0400`. The
  configuration directory is already denied to every agent, so no session reads another
  project's file. The daemon reads it at each launch, so a replaced token reaches the next
  launch without a restart. A file convention, not an owner setting naming a path: there is
  nothing to parse, and the token cannot sit in a repository or a world-readable place by
  mistake. `cockpit_config::read_github_token` owns the rules.
- **A wrong file refuses the launch, never weakens it.** A symbolic link, a folder, a file
  whose mode is not `0600` or `0400`, an empty file or one that holds anything but one token (letters,
  digits, `_`, one line) fails the project's launches with the path and the fix. It is not
  skipped (that would silently fall back to the owner's login) and not used.
- **Only `GH_TOKEN` reaches the agent, and only its project's.** Every launch drops
  `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN` and `GITHUB_ENTERPRISE_TOKEN`, whatever
  the daemon or a provider passed, then sets `GH_TOKEN` when the project has a token. The
  value travels in the launch's private environment file, never in argv; greywall gets
  `--ignore-secret GH_TOKEN`, the name alone.
- **The token passes through greywall, it is not a greyproxy placeholder.** Git sends it
  inside HTTP Basic credentials (`gh auth git-credential`), base64-encoded, and greyproxy
  replaces placeholders only as plain text in headers and query strings, so `git push` would
  carry the placeholder. The token is limited to one repository (contents and pull requests),
  which bounds what an agent that reads it can do.
- **With a token, the owner's `gh` login is denied to the agent:** `~/.config/gh` (which
  greywall's base profile lets every agent read) and `$XDG_CONFIG_HOME/gh`, so `gh` and Git
  use the project's token alone. Without a token they stay readable, as Theo decided for
  projects that have none yet.
- **Torlek says whether a project has a token, never what it is.** `torlek project show
  PROJECT` prints `github-token` `configured`, `none` or `unusable` with the file or the
  reason; the project list carries the same state (serde-optional, so an older daemon's reply
  still parses). The brief says, as the file reads when it is built, that the project has its own
  token set as `GH_TOKEN`, or none so Torlek sets no `GH_TOKEN`.
- **Rejected:** a placeholder through greyproxy (breaks `git push`, above); reading the
  token from the owner's keyring (the daemon would need the session bus, and the keyring
  holds the owner's own login, not a per-project one); a short-lived token minted per
  session (that needs a GitHub App, which this issue does not set up).
- **Residual:** a `GH_CONFIG_DIR` the owner set for themselves is not denied, since agents do
  not get that variable and greywall reads nothing outside the paths it allows; a Git
  credential the owner stored elsewhere (`~/.git-credentials`) is not covered by this issue.
