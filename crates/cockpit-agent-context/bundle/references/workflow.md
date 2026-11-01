# Workflow: your task's steps

A task follows a workflow preset (`cockpit workflow presets`). Each step has a doer (an agent
or the owner), requirements, and the provider capabilities it needs.

- `cockpit workflow list` shows your project's runs; `cockpit workflow packet ID` rebuilds
  everything you need for the current step from durable records.
- `cockpit workflow begin ID` takes the ready agent step. One step at a time per session.
- `cockpit workflow checkpoint ID NOTE` records progress. The latest checkpoint survives a
  restart, a release and a new session: whoever continues starts there.
- `cockpit workflow complete ID --changed T --checked T --outcome T [--needs-human T]
  [--evidence REF]...` ends the step with a handoff. Each requirement must be satisfied by
  evidence (see `checks`) or waived by the owner; prose never meets one.
- `cockpit workflow release ID --changed T --checked T --outcome T` hands the step back
  mid-way, with what you did.
- `cockpit workflow block ID ACTION --detail TEXT` stops the run on one next action:
  `answer-question`, `approve-operation`, `restore-capability`, `wait-for-capacity` or
  `reconcile-result`. `cockpit workflow unblock ID` once it is resolved.

A handoff is at most 2 KiB: what changed, the exact result checked, what passed or failed,
what still needs the human. Reference evidence, one line each; never paste logs.
If the goal's intent changed since the run started, the packet says so: re-read it before
relying on earlier approvals.
