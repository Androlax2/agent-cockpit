# cockpit-scheduling

Owns what runs where and why it waits: which waiting work is eligible, which session takes
it, project-then-task fairness with aging, deterministic dispatch, and an explanation record
per waiting item. Design: [report 10](../../docs/design/greenfield/10-resources-scheduling.md)
and [report 06](../../docs/design/greenfield/06-tasks-workflows-verification.md).

It never depends on the Chief: the Chief proposes, scheduling places (report 07).

## What waits

Every open item of `cockpit-tasks` is in exactly one of these:

| The item | Waits as | Explained as |
| --- | --- | --- |
| follows a running workflow whose next step is a ready agent step | that step | the queue's reasons below |
| follows a blocked run, or one whose next step is the owner's | — | `run-blocked`, `owner-step` |
| has no run, is `todo`, queued and waits on nothing unfinished | the item | the queue's reasons below |
| has no run and waits on unfinished items, or is not queued | — | `waits-on-items`, `not-queued` |

Items being done (an active step, a `doing` item without a run) are not waiting.

## One pass

`dispatch` runs in the caller's write transaction, so what it reads, the work it takes and
the records it writes commit together. The daemon runs one every 30 seconds and on
`cockpit queue dispatch`; no Chief turn is involved (report 10, "routine dispatch").

1. **Settle.** A placement ends when its session no longer does its work (the item is not
   `doing` by it, the step not `active` by it), or when its session is gone (ended).
2. **Order.** Projects take turns: the project with the fewest active placements goes first,
   then the one whose next unit waited longest, then by name. A project's share counts its
   placed work, never its sessions or checkouts, so fan-out never raises its entitlement.
   Inside a project the highest aged priority goes first (the item's priority plus one level
   per 15 minutes waited, at most 4), then the oldest, then the lowest item number.
3. **Admission.** An agent turn (500 millicores, 1 GiB) is checked against
   `cockpit-resource-admission`: a paused queue, a required control or a demand that never
   fits holds it with admission's reason. Room is counted against the budget left after
   admission's reservations and the active placements; each placement charges its demand.
4. **Startup headroom.** A project no placement has finished in yet is unfamiliar: its turns
   are charged 50 % more until one finishes with its work done.
5. **Gathering.** A unit held for room that has waited 30 minutes gathers capacity: units
   ordered after it that would start are held `behind` it, so released capacity accumulates
   until the large job starts. Units held for another reason do not take capacity and keep
   their own explanation.
6. **Eligible session.** A session of the unit's project takes it when, in this order:
   its role matches (a step naming a role goes only to that role; a session holding a role
   takes only its role's steps), its provider reports the step's capabilities, it does no
   other item or step and may change its own state, it runs, its current launch's agent
   reported waiting at its prompt and asks nothing, the owner is not viewing it, and the idle
   window (5 minutes since a pass first saw it waiting) has passed. The one waiting longest
   takes it.
7. **Place.** A queued item is started by the session (`cockpit-tasks` `start`), a ready
   step begun by it (`cockpit-workflows` `begin`), and the placement recorded. When the
   session goes, its item returns to the queue and its step to ready.

Roles come from `cockpit-teams`; until it exists no session or step holds one.

## Explanations

Each pass rewrites one record per waiting item, in dispatch order: the reason (a stable
name), the sentence, what limits it (a resource and its capacity, a control on this host),
each session of the project and why it cannot take it, the item it waits behind, what it
would charge (headroom included), its priority and aging, when it entered the queue, when it
was evaluated and when the next pass looks again, and the available action with who does it
(`owner`, or `cockpit` when time or capacity moves it). `cockpit queue explain [ITEM]` prints
them.

## Durable state

`scheduling_waiting` (when each unit entered the queue, so aging survives a restart),
`scheduling_placements` (one active per session and per item) and
`scheduling_explanations`. Nothing is kept in memory between passes.

## Dependency edges

- Depends on: `cockpit-tasks` (the items, `start`), `cockpit-workflows` (the runs, `begin`,
  step capabilities), `cockpit-resource-admission` (holds, the budget's reservations),
  `cockpit-authorization` (whether a session may change its own state),
  `cockpit-provider-api` (capabilities), `cockpit-turso`.
- Never on the Chief, sessions, tmux or the wire types: the daemon passes the sessions as
  [`Candidate`]s and converts to wire types (`cockpit-workspace-rules/tests/crate_graph.rs`,
  rule `scheduling -> chief/sessions/wire types`). No other capability depends on it (rule
  `capabilities -> scheduling`).
- Used by: the daemon.

## Tests

- `tests/fairness.rs`: three projects sharing the budget take turns whatever their session
  count; aging; gathering; startup headroom.
- `tests/eligibility.rs`: idle window, viewed session, roles, capabilities, busy sessions,
  workflow steps, settling.
- `tests/explanations.rs`: every reason with its action, and records that survive a reopen.
- Through the socket: `cockpit-daemon` `tests/scheduling.rs` and `cockpit-cli`
  `tests/queue.rs`.
