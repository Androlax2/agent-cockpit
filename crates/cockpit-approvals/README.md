# cockpit-approvals

Owns the owner's decisions on what agents ask the host to do: each command bound to its
exact digest, working directory, revision and expiry, consumed once by the run it
authorizes, and the guides walked one step at a time.

Report 17. An agent asks with `cockpit ask run` or `cockpit ask guide` on the agent channel;
the daemon identifies the session (`cockpit-agent-channel`) and keeps its own copy of the
request here, so nothing is re-read from the sandbox.

## Run requests

1. `ask_run` files the request after `cockpit-authorization` allows the session
   `run-commands` over itself. The working directory is resolved first (`resolve_cwd`), and
   the shell must parse the command (`Runner::check_syntax`, `<shell> -n -c`).
2. The owner reads it through `show`: hiding characters escaped as `\u{…}`, runs of blank
   lines collapsed, non-ASCII characters named, a directory outside the asking agent's
   project flagged (`RunRequest::is_outside_project`).
3. `approve_run` needs the request's current revision and its `action_digest` (SHA-256 of
   the directory and command). It consumes the request with the run intent in one
   transaction; a second decision, an older revision or another digest is refused. The
   owner's `edit_run` creates a new revision. Asking requests expire
   `EXPIRES_AFTER_SECONDS` after the agent last asked about them (`run_status`).
4. `Runner::run` runs the approved revision in a pseudo-terminal through `run_leader`
   (a session leader, so prompts and `sudo` work), Git hooks off and program-running Git
   settings reset, and returns the end of the output without terminal controls and with
   secrets redacted (`present`). `start_run` and `finish_run` are fenced on the revision.
5. `reconcile_runs` at startup: an approved run never started is `not-run`, a running one
   `outcome-unknown`. Neither is repeated.

`queue_order` puts what the owner is asked in order: a running command waiting for input
first, then asking requests oldest first.

## Guides

`parse_guide` reads `# Title`, `## Section`, one step per line and indented `open:`,
`copy:`, `ask:`, `run:` lines. The owner answers the current step only (`answer_step`,
bound to its index), reports being stuck there (`report_stuck`) or closes the guide. A
`run:` step files its command as a run request of the guide's session.

## Dependency edges

- Depends on: `cockpit-turso` (transactions, migrations) and `cockpit-authorization`
  (every ask is evaluated before it is filed).
- Never on `cockpit-protocol`, a client, `cockpit-agent-channel` or `cockpit-projects`:
  the daemon identifies the sender, finds the project and converts to wire types
  (`cockpit-workspace-rules/tests/crate_graph.rs`).
- Used by: the daemon.

## Tests

- `tests/display.rs`, `tests/redaction.rs`: the display and redaction rules.
- `tests/runs.rs`: the run state machine on a real state database, report 04's
  stale-approval walkthrough included.
- `tests/guides.rs`: the guide format and step answers.
- The pseudo-terminal runs are proven end to end in `cockpit-daemon/tests/approvals.rs`.
