# Ending a failed or launching session stops its surviving agent first

Issue #221. Decided by the worker, with the reasons below.

- **`end` from `launching` goes through the stop, as from `running`.** The session moves to
  `stopping` (ended), its latest execution is terminated and observed gone, then it is
  `ended`. Same path `restart` takes from `launching` since #157; a crash midway is finished
  by restore as any other stop.
- **`end` from `failed` stops the latest agent in place, as `resume` does.** The previous
  agent is terminated and observed gone before the move to `ended`; one not observed gone
  refuses the end with `StopUnconfirmed` and the session stays `failed`.
- **Rejected:** `failed` through `stopping` too. A stop that cannot be confirmed puts the
  session back to `running` (#31); a session that never had a confirmed agent (its launch
  failed before the agent was identified) would then be shown running with nothing behind it.
- Ending a `failed` session now needs tmux, to close its terminal, as every other stop does;
  without tmux the end is refused naming tmux, before anything changes.

Evidence: `crates/cockpit-daemon/tests/session_end_failed.rs`.
