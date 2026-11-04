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
- **Known window:** a session counts from its row, which `started_by` marks when the start
  creates it. Between an unconfirmed start's audit and that row, a second start the same
  coordinator asks is not counted against it. The hourly cap still bounds it.
- **Rejected:** counting from the audit alone (`start` minus `declined` and ended `started`
  rows): a request that expires or is not run leaves no row to match, and the rows link only
  through their text.
