# Exercise the packaged TUI through its CLI sibling lookup

Issue #493. Decided by the worker within the approved headless packaging smoke scope.

- **Use `cockpit tui --version`.** The CLI executes `cockpit-tui --version` beside itself,
  preserving the normal sibling lookup and process exit status. The TUI prints its compiled
  package version without a terminal, daemon, configuration or state writes.
- **Run it from the unpacked archive.** `tools/release.py verify-install` checks the TUI
  version against the manifest, in addition to checking every bundled file's checksum.
  The release matrix builds and gathers the TUI, including both macOS universal slices.
- **Rejected:** opening a tmux manager or adding screenshot functionality to verify packaging.
  Those introduce terminal lifecycle or UI behavior beyond this fix.
