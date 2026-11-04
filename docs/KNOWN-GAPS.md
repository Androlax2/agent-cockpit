# Known gaps

This is an alpha. Data may be reset between alpha releases. Fixes for the gaps below are planned for `0.1.0-alpha.2`.

- **Linux and WSL2 only, from the release archive.** There are no macOS builds (#568). Arch, Debian/Ubuntu and Scoop packages are prepared but not published yet (#562, #563, #565).
- **A sandboxed agent's request to a new website waits until it times out.** You are not asked to allow or deny it yet (#641).
- **A session whose agent dies right at start can show as running for a few seconds in the TUI**, then disappear. `torlek session list --ended` shows its exit status and last output (#632).
- **A session that has not had its first turn yet** cannot sleep, wake or restart, and messages to it stay queued (#647, #656).
- **Some help messages point to the wrong next step:** a paused queue (#646), `todo propose` from your own shell (#648), and `check gate` on a project you have not trusted (#649).
- **`approval show` does not say who approved a request or when** (#657).
- **The support bundle** has no per-component version list or time window yet (#652).
