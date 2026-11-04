# A start-sessions grant caps the sessions its holder runs at once, beside its starts per hour

Issue #739. What counts as active and the default of 8 follow the issue's Expected and its
recommendation; the rest was decided by the implementing session.

- **`StartTerms.max_active` bounds the load; `per_hour` stays as a runaway guard.** Starts per
  hour measured how often a coordinator starts, not how many workers run: a short task held
  its start for the full hour, and long workers piled up. Both caps apply; `max_active` is
  checked first, and the refusal names the cap hit (`AtCapacity { max_active, active }` or
  `Capped`).
- **Active means started and not ended, or waiting for the owner.** The sessions whose
  `started_by` (#710) names the holder, or that its `started` audit entries name (sessions
  started before #710 have no `started_by`), whose state is not `ended` (asleep and failed
  ones count: a wake or a resume brings them back), plus its start requests still open in the
  `ask run` queue. Ending one frees a place at once.
- **The daemon counts; `cockpit-authorization` decides.** Sessions and approvals own the
  states, and the authorization crate depends on no product capability, so
  `crates/cockpit-daemon/src/coordinator_workers.rs` counts in the start's write transaction
  and passes the count to `start_allowance`.
- **Defaults: 8 at once, 20 an hour.** Migration 4 gives every stored `start-sessions` grant
  `max_active` 8, so `session grants` shows the cap it works under; stored grants keep their
  own `per_hour`.
- **A start under way counts before its session exists.** A start without confirmation commits
  its audit before `Sessions::start` creates the session's row, so parallel starts would all
  read the same count. The daemon keeps those starts in memory (`Launching`), counted from
  before the audit commits until `Sessions::start` returns; a start under way does not
  outlive the daemon.
- **Regranting replaces every term.** `session grant ... start-sessions` without
  `--max-active` gives the default, as without `--per-hour`; the reply prints the terms now
  in force.
- **A refusal at capacity names the owner as the one who unblocks it.** Asleep and failed
  sessions count until the owner ends them; a coordinator has no verb to end one.
- **Rejected:** counting from the audit alone (`start` minus `declined` and ended `started`
  rows): a request that expires or is not run leaves no row to match, and the rows link only
  through their text.
