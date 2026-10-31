# Gate M1: live acceptance runs (#135)

Proof for [gate #37](https://github.com/Androlax2/agent-cockpit/issues/37)'s "live acceptance
completed by Theo". The guide is [docs/acceptance/m1.md](../acceptance/m1.md), in
`ask-to-guide` step format; it is re-run after fixes and each run is appended below.

## How it runs

- Build this clone: `cargo build --release --manifest-path Cargo.toml --bin cockpit --bin
  cockpit-daemon --bin cockpit-tui --bin cockpit-claude-hook`.
- From the clone root, the worker hands the guide over: `ask-to-guide < docs/acceptance/m1.md`.
  Its `run:` steps call `tools/acceptance/run.sh`, which runs `target/release` on scratch
  `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `XDG_CACHE_HOME`, `XDG_RUNTIME_DIR` and `TMUX_TMPDIR`
  under `/tmp/cockpit-acceptance-<uid>`, never the installed cockpit. Until #134 lands this is
  the only isolation; from then on the `cockpit-next` profile replaces it.
- `HOME` stays the owner's, so sessions use the owner's Claude Code login and its user-level
  hooks; the agent runs the first `claude` on `PATH`, unsandboxed (sandbox profiles come later),
  in the scratch project `demo`.
- Every stuck step or surprise becomes an issue (label `acceptance`, milestone M1). The gate
  closes when each is fixed and its step re-run, or deferred in writing on #37.

## Runs

### 2026-10-31, worker dry run (not Theo's walk)

- Commit `5e1164c8d19446e53e4cdcb7175ac4e00afdf9a3`, Linux, inside the worker's sandbox, a
  stand-in `claude` (prints a version, runs `cockpit agent report`, `ask run`, `ask guide`).
- 1, install and scratch home: pass. Every path under the scratch directory; idle window 30
  minutes from the scratch config.
- 2, sessions in place, in a clone (`--clone` and the `N` form): pass. On attach the viewer
  has focus, so the list ignores keys until clicked: #271.
- 3, restart, sleep, wake, exit, resume, `kill -9` and restart: pass. The TUI showed
  "cannot connect" while the daemon was down and reconnected on its own; sessions reattached.
- 4, `ask run` / `ask guide`: blocked. No session holds `run-commands`: #270 (decided A, to
  implement); and #268 below.
- 5, agent reports: blocked. A daemon-launched agent's `cockpit` dials
  `~/.local/state/cockpit/run/agent/agent.sock` because `XDG_RUNTIME_DIR` is not passed: #268.
  With the variable forced in the stand-in, label, doing and needs-input reached the card.
- 6, latency feel and idle sleep: needs Theo; not measured in the dry run.
