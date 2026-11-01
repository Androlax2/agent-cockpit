# Scheduling places work as durable placements it charges itself, project by project

Issue #43. Decided by the implementing session, within report 10's proposals (fairness,
aging, startup headroom, gathering, explanations) and report 06's split of tasks and
workflows.

- **A placement charges the budget, not an admission reservation.** Admission releases a
  running reservation only once it observes its execution end, and one execution holds one
  reservation; an agent's turns run inside one long-lived execution, so per-turn
  reservations do not fit. A pass checks an agent turn with admission's `check` for its holds
  (paused queue, controls, a demand that never fits) and counts room itself: the budget less
  admission's reservations less the active placements. Builds admitted by admission do not
  see placements yet; that gap is filed as a leftover.
- **Fairness counts placed work, never sessions.** The project with the fewest active
  placements goes first, then the one whose next unit waited longest, then by name. A project
  with ten sessions gets no more turns than one with one session.
- **Aging and gathering use the time a unit entered the queue,** kept in
  `scheduling_waiting` so a restart keeps it: one priority level per 15 minutes, at most 4;
  a unit held for room for 30 minutes holds the units ordered after it (`behind`) until it
  starts.
- **Startup headroom is 50 % until a placement finishes in the project.** A project is
  familiar once one of its placements ended because its work ended; a session that vanished
  does not count.
- **What waits is the item, or its run's ready agent step.** An item a running workflow
  follows waits as that step, whether or not the item is queued: starting the workflow is
  the owner's choice to have it done. Blocked runs and owner steps are explained, not placed.
- **The viewed session is the one last shown in the manager's viewer.** The daemon does not
  know whether a terminal is attached, so it keeps the conservative answer: the last shown
  session gets no new work until another is shown.
- **Roles route only once teams assign them.** A step naming a role goes only to a session
  holding it, and a session holding a role takes only that role's steps; until
  `cockpit-teams` (#53) exists, neither holds one.
- **A pass is one write transaction.** It reads the items, runs and placements, takes the
  work for the session (`cockpit-tasks` start, `cockpit-workflows` begin), records the
  placement and rewrites every explanation together, so two passes never place the same unit.
- **Placing does not deliver.** The pass records that the session does the work; sending it
  to the agent is filed as a leftover.

Rejected:

- Keeping the queue in memory: aging and explanations would reset on every restart.
- Counting sessions or checkouts for a project's share: fan-out would buy entitlement.
