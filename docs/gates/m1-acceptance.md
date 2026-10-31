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
- `run.sh tui-window` opens the TUI in its own kitty window (class `cockpit-acceptance`) after a
  Hyprland rule makes that class opaque: Theo's default window opacity was too faint to read.
- `HOME` stays the owner's, so sessions use the owner's Claude Code login and its user-level
  hooks. `run.sh start` links the first `claude` on `PATH` outside a `greywall-agents`
  directory (or `COCKPIT_ACCEPTANCE_CLAUDE`) into the scratch `bin`: the greywall launcher would
  hide this clone's `target/release` from the agent. The agent runs unsandboxed (sandbox
  profiles come later), in the scratch project `demo`.
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

### 2026-10-31, worker dry run on main after the fixes

- Commit `3fdce9b` (with #281, #283, #287, #288 and #292), a stand-in `claude`, scratch home
  `/tmp/acc-dry`.
- 1 to 5: pass. `ask run` and `ask guide` reach the owner and return; label, doing and
  needs-input reach the card.
- 6, idle sleep: the stand-in that reports needs-input never sleeps, by design (the turn must
  end Waiting without needs-input). An idle-only stand-in, watched from 23:18, was asleep by
  23:47: pass.

### 2026-10-31, Theo's live walk

- Commit `3fdce9b`, Theo's Hyprland desktop, real Claude Code, in three hand-overs.
- First hand-over: stuck on section 2, step 1: the step said to open the TUI without the
  command. The guide was rewritten so every step names the keys, clicks and commands, with a
  `copy:` line for the TUI command.
- Second hand-over: 1 to 3 pass. 4 failed: the agent's `claude` was the greywall launcher, which
  hid `target/release` (hook "No such file", `cockpit: command not found`). `run.sh start` now
  links a `claude` outside greywall.
- Third hand-over (sections 4 to 6): 4 pass: the Waiting on you pane showed the command,
  directory and reason; the approved run returned with `GITHUB_TOKEN=[redacted]`; the declined
  one returned exit 125; the guide's answer reached the agent. 5 pass: `!`, waiting on you, label
  and doing on the card, `n` opened the session, and the marks cleared after the answer.
- 6, latency: "pretty fast", nothing over a second. Idle sleep: not verified; Theo did not wait
  the 30 minutes.
- Theo's notes:
  - The tmux status bar (green, at the bottom of the viewer) shows all the time; it should go:
    #302.
  - Next guides should turn off the cockpit window's opacity on Hyprland first: it was hard to
    see anything. Theo's setup, not Cockpit: handled by `run.sh tui-window` (see above).
- Seen in the screenshots: the owner's Claude Code stop hooks fail to notify ("Cannot autolaunch
  D-Bus without X11 $DISPLAY"): daemon-launched agents get no `DISPLAY` or `WAYLAND_DISPLAY`:
  #303.
