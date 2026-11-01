# The basic TUI's workflow view opens on `F` and completes a step only by waiving it

Issue #330. Decided by the worker, within decision 17 (basic TUI until M6).

- **`F` opens the running workflows.** No other key had this meaning in the legacy TUI, and
  the keys decision 17 dropped (`w` among them) stay dropped. Inside the view, `a` complete,
  `p` release, `b` unblock, `x` cancel; release and cancel need `y`, as restart and exit do.
- **There is no item list in the TUI yet (#44)**, so the view lists the running workflows
  and opens one; the item's own view can open it once #44 lands.
- **Completing from the TUI waives every requirement of the current step.** The owner types
  the handoff (what changed, what was checked, the outcome); the TUI cannot report
  verification, so a step with requirements completes only by the owner's waiver, as on the
  CLI (2026-11-01-39). The daemon still refuses what the owner may not do.
- **While open, the view is refreshed with the list** (`workflow-list` and the opened
  workflow's `workflow-packet` each second), so an agent's progress and an action's effect
  show without a key.
- **Not designed in FigCraft:** the plain TUI is designed in M6 (decision 7), so no design
  was asked for.
