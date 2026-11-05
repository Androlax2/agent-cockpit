# Verification and CI changes

- Make recipes own executable check commands. CI should call those same targets;
  documentation links to them rather than maintaining another implementation.
- Work under `crates/` runs cargo directly, never `agent-build`, until the cutover (#66);
  see CLAUDE.md, Tests. For `apps/`, preserve `agent-build` scheduling and the shared
  full-test lock. Focused tests can run independently; full runs use `make test`.
- Preserve the OSD build prerequisite: tests use the real classifier executable.
- Use the committed Cargo lockfile. Dependency updates are explicit, approved changes.
- Missing prerequisites and skipped environment-specific tests must be visible in
  results. Never call a partial run full verification.
- Keep ordinary CI read-only with respect to repository permissions. Installing a
  hook, activating a workflow, pushing it or changing required checks is a separate
  operation subject to the user's authorization.
- Tests and tools never reach tmux's default socket directory (`/tmp/tmux-<uid>`), where
  the owner's installed Torlek runs: name a tmux server by path (`tmux -S`) under the test's
  own directory, never by `-L` (it falls back to `/tmp` when `$TMUX_TMPDIR` is missing);
  give every daemon a test starts a `TMUX_TMPDIR` that exists, or put a fake `tmux` on
  `PATH`. `crates/cockpit-workspace-rules/tests/tmux_sockets.rs` checks it (#845).
- Keep test discovery and tool selection deterministic across local and CI execution.
  Include extensionless scripts and avoid silently excluding new workspace members.

[Development](../../docs/DEVELOPMENT.md) describes current commands. The engineering
alignment plan contains proposed targets; they are not available until implemented.
