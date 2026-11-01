# cockpit-workflows

Owns the next required step of a task and its progress: versioned presets with the provider
capabilities and requirements each step declares, step state, checkpoints, handoffs,
blockers, and the task packet rebuilt from durable records. Design:
[report 06](../../docs/design/greenfield/06-tasks-workflows-verification.md) and the packet
fields of [report 08](../../docs/design/greenfield/08-agent-onboarding-context.md).

It never places work (`cockpit-scheduling` consumes [`ready_steps`]), never evaluates
evidence (`cockpit-verification` decides which requirements passed; the caller passes their
names to [`complete`]) and never launches agents.

## Presets

| Preset (version) | Steps (doer) | Requirements |
| --- | --- | --- |
| `research` (1) | investigate, synthesize, deliver (agent) | none |
| `code-change` (1) | implement, check, deliver (agent) | `check`: `checks`, `review` |
| `design-led` (1) | design-approval (owner), implement, verify (agent) | `verify`: `checks`, `design-match` |

Every agent step requires the provider capabilities `events` and `outcome`: a prompt the
agent accepted is not work it completed. A run keeps the preset version it started with;
reading a run whose version this release no longer has fails loud.

The preset a task follows: the one the owner names, else the project's `workflow.preset`
(`cockpit-config`), else `research` for a research item and `code-change` for any other
([`select_preset`]). A name no preset has is refused, never replaced by the default.

## Runs and steps

| Step status | Means | Leaves by |
| --- | --- | --- |
| `pending` | an earlier step is not done | the step before it completing |
| `ready` | the next required step | `begin` by a session (agent step); the owner's `complete` |
| `active` | a session is doing it | `complete` (`done`), `release` (`ready`, checkpoint kept) |
| `done` | handed off | never |

- **Start.** The owner starts one running workflow per open (`todo`, `doing`) item; the run
  records the item's intent revision, and the packet says when the intent has changed since.
- **Begin.** A session of the run's project begins a ready agent step when its provider
  reports every capability the step declares ([`check_capabilities`]; a discovered,
  untested capability counts, as for a launch). A session does one step at a time.
- **Checkpoints.** The session doing the step records its progress; the latest checkpoint
  survives the session, a release and a daemon restart, and the next session resumes there.
- **Complete.** Ends the step with a handoff once each requirement is satisfied (passed, as
  the caller's verification says) or waived by the owner. The next step becomes ready; after
  the last, the run is `delivered`.
- **Release.** The session doing the step hands it back mid-step with a handoff; the owner
  may release a session that cannot, without one.
- **Blockers.** The session doing the step or the owner blocks the run on one next action:
  `answer-question`, `approve-operation`, `restore-capability`, `wait-for-capacity` or
  `reconcile-result`, with what exactly is needed. A blocked run's step is not placed or
  completed until it is unblocked.
- **Cancel.** The owner stops a run; the item may start another.

## Handoffs

A handoff names what changed, the exact result that was checked, what passed or failed, what
still needs the human (may be empty), and references to its evidence. Its prose is at most
`HANDOFF_MAX_BYTES` (2 KiB, report 06); evidence is referenced, one line each, never pasted.
A field that only claims success ("done", "fixed") is refused. A requirement is never met by
prose: only the satisfied names the caller passes, or the owner's waiver, count.

## Task packet

[`packet`] rebuilds, from durable records only, what a session receives: the goal (item
title, notes, kind, status, intent revision then and now), the project as authorized scope,
the preset, the current step with its requirements, attempt, session and checkpoint, the next
action, the blocker, the summaries of the items it waited on, and every handoff so far. Grants
are not in it: they are evaluated at every mediated action. The launch brief
(`cockpit-agent-context`) adds the instruction-discovery entry point.

## Who may do what

The owner does everything. A session needs `change-state` over itself, granted at launch;
with it, it sees its own project's runs and their packets, begins their ready agent steps,
and checkpoints, completes, releases, blocks and unblocks the step it is doing. Starting,
cancelling, completing owner steps and waiving requirements are the owner's.

## Dependency edges

- Depends on: `cockpit-turso` (transactions, migrations), `cockpit-authorization` (every
  session request is evaluated), `cockpit-tasks` (the item a run follows) and
  `cockpit-provider-api` (the capabilities a step declares).
- Never on scheduling, sessions, verification, process launching or the wire types
  (`cockpit-workspace-rules/tests/crate_graph.rs`, rule
  `workflows -> sessions/verification/processes/wire types`).
- Used by: the daemon.

## Tests

- `tests/presets.rs`: preset selection, versions and capability checks.
- `tests/runs.rs`: the step state machine, handoffs, requirements, blockers and who may act.
- `tests/restart.rs`: a run reopened from its database resumes at its checkpoint with the
  same packet.
- Through the sockets: `cockpit-daemon` `tests/workflows.rs` (a daemon killed mid-step
  answers the same packet after its restart) and `cockpit-cli` `tests/workflow.rs`.
