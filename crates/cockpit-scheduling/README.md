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
| would wait as its step or itself, but its sessions have worked it for its budget | — | `over-budget` |

Items being done (an active step, a `doing` item without a run) are not waiting.

An item's budget (`budget_minutes` in `cockpit-tasks`, #412 owner choice 2A) counts the agent
seconds the daemon passes in `Inputs::agent_seconds`: the turns of each session it was placed
on while that placement held (`work_spans`). Placed work runs on past it; the queue only stops
placing the item again or its run's next step, and the action offers 30 minutes more. The
owner's handoff is their answer, so a budget never holds it. The daemon tells the owner once,
when a turn ends past the budget (`budget-spent` on the change feed).

## One pass

`dispatch` runs in the caller's write transaction, so what it reads, the work it takes and
the records it writes commit together. The daemon runs one every 30 seconds and on
`torlek queue dispatch`; no Chief turn is involved (report 10, "routine dispatch").

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
   admission's reservations, which include every placement's (step 8).
4. **Startup headroom.** A project no placement has finished in yet is unfamiliar: its turns
   are charged 50 % more until one finishes with its work done.
5. **Gathering.** A unit held for room that has waited 30 minutes gathers capacity: units
   ordered after it that would start are held `behind` it, so released capacity accumulates
   until the large job starts. Units held for another reason do not take capacity and keep
   their own explanation.
6. **Eligible session.** An item the owner marked economy (`is_economy` in
   `cockpit-tasks`, #412) looks only at the sessions of its project started with the
   project's `Economy` model and effort (`Inputs::economy`, from the owner's
   `[usage.economy]`); each other session is noted. Without such a session, or without an
   economy for the project, it waits `economy` with the `session new` flags to start one;
   the queue never starts or relaunches a session. The owner's handoff ignores the mark.
   A session of the unit's project takes it when, in this order:
   its role matches (a step naming a role goes only to that role; a session holding a role
   takes only its role's steps), its provider reports the step's capabilities and `submit`
   (the work is typed to its agent), it does no
   other item or step and may change its own state, it runs, its current launch's agent
   reported waiting at its prompt and asks nothing, the owner is not viewing it, and the idle
   window (5 minutes since a pass first saw it waiting) has passed. The one waiting longest
   takes it.
7. **Pace.** Of the eligible sessions, one whose account's plan windows are ahead of the
   pace does not take it (#412, below). When every eligible session is paced, the unit
   waits `usage-paced`.
8. **Place.** A queued item is started by the session (`cockpit-tasks` `start`), a ready
   step begun by it (`cockpit-workflows` `begin`), its agent turn held as an admission
   reservation (`hold`, #386) that the placement's end releases, the placement recorded and
   its delivery enqueued in `cockpit-messaging`'s outbox (#385). When the session goes, its
   item returns to the queue and its step to ready.

## Pacing

`Settings::pacing` keeps queue starts to an even spend of each plan window (#412, owner
decision 2026-11-09). The daemon passes every account's windows as `cockpit-usage` judged
them, and each session's provider and account (an agent on no account runs on the owner's
own sign-in, read as `default`). Under `Pacing::Even` (reserve 10 %, burst 15 % by default)
a session takes queued work only while each whole-account window of its account is used
below its line:

```text
line = min(elapsed share of the window × (100 − reserve) + burst, 100 − reserve)
```

| Work | Line |
| --- | --- |
| priority 1 (low) | no burst: it waits first |
| priority 2 | the line above |
| priority 3 and up | `100 − reserve` only |

A window the provider blocks, scoped or not, holds every priority; a stale window, one scoped
to a model and one of another account pace nothing. Aging orders the queue, but pacing reads
the item's own priority: an aged low-priority item never earns the burst. Of several windows holding a session, the one that
holds it longest explains it; of several paced sessions, the one that frees first names the
reason and when the line reaches the window's share (or the reset, at the reserve). Each
paced session is noted with its own window. Running work is never paused, so what it spends
after the last start comes out of the reserve. A handoff is the owner's and is never paced.

A project the owner gives a share (`UsageWindow::project_shares`, #412 owner choice 1A) gets
no queue start on an account once its estimated part of one of the account's current,
whole-account windows reaches the share: every priority, whatever `Settings::pacing`, until
the window resets. The daemon estimates the part as the window's share used times the
project's API-price cost since the window opened over the account's. The explanation is the
same `usage-paced`, limited by the project's share; of pacing and a share, the one holding
longer explains it.

`window_pace` is one window's standing against the line for normal-priority work: the line
now, whether it holds the queue, and when it lets work start. The daemon adds it to each
window `usage-quota` answers.

## Delivery

A placement's envelope (producer `scheduling`, effect key `placement-<id>`, destination the
session) carries one line, `Placement::brief`: for an item, where to read it
(`torlek todo show`) and how to finish it; for a step, its task packet
(`torlek workflow packet`). The work itself is read from its durable records, so the line
never goes stale. The daemon types it to the agent after the pass commits. Typing twice
would start the work twice, so the envelope is never redelivered after an unconfirmed
attempt; a delivery that fails for good (`delivery_failed`) ends the placement
`delivery-failed` and puts its work back in the queue, where a later pass places it again.

Roles come from `cockpit-teams`; until it exists no session or step holds one.

## Explanations

Each pass rewrites one record per waiting item, in dispatch order: the reason (a stable
name), the sentence, what limits it (a resource and its capacity, a control on this host),
each session of the project and why it cannot take it, the item it waits behind, what it
would charge (headroom included), its priority and aging, when it entered the queue, when it
was evaluated and when the next pass looks again, and the available action with who does it
(`owner`, or `cockpit` when time or capacity moves it). `torlek queue explain [ITEM]` prints
them.

An open item without a record began waiting after the last pass; `awaits_pass` tells it
from one that is placed, done or gone, so `queue explain` can say the next pass considers it.

## Durable state

`scheduling_waiting` (when each unit entered the queue, so aging survives a restart),
`scheduling_placements` (one active per session and per item, each with its admission
reservation) and
`scheduling_explanations`. Nothing is kept in memory between passes.

## Dependency edges

- Depends on: `cockpit-tasks` (the items, `start`), `cockpit-workflows` (the runs, `begin`,
  step capabilities), `cockpit-resource-admission` (holds, the budget's reservations),
  `cockpit-authorization` (whether a session may change its own state),
  `cockpit-provider-api` (capabilities), `cockpit-messaging` (the delivery outbox: a
  producer depends on messaging, never the reverse), `cockpit-turso`.
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
- `tests/delivery.rs`: the enqueued line for an item and a step; a failed delivery hands the
  work back.
- `tests/pacing.rs`: the line by priority, the reserve, blocked and unpacing windows, the
  explanation, and a recorded feed that full speed spends by half the window and the
  default pace makes last until the reset; each window's pace as `usage quota` shows it.
- `tests/project_shares.rs`: a project at its share waits whatever its priority and the
  pacing, its explanation, and the windows and projects a share does not cap.
- `tests/budget.rs`: an item and a run's next step at their budget wait for the owner, its
  explanation, and an item without a budget or not queued.
- `tests/economy.rs`: an economy item goes only to a session started with its project's
  economy, waits `economy` without one or without a project economy, and is handed over
  anywhere.
- `tests/charges.rs`: placed turns as held reservations that builds and `in_use` see.
- Through the socket: `cockpit-daemon` `tests/scheduling.rs` and `cockpit-cli`
  `tests/queue.rs`.
