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
   lines collapsed, each kind escaped named in `Shown::warnings` (`Hidden`), non-ASCII
   characters named, a directory outside the asking agent's project flagged
   (`RunRequest::is_outside_project`). `fold` gives a long command's start and what it
   leaves out, for a pane's explicit "more". `timeout_without_foreground` and
   `build_files_run` (the changed build files and scripts a command runs, from the
   checkout's changed files the daemon reads) say what the text alone does not (#156).
3. `approve_run` needs the request's current revision and its `action_digest` (SHA-256 of
   the directory and command). It consumes the request with the run intent in one
   transaction; a second decision, an older revision or another digest is refused. The
   owner's `edit_run` creates a new revision. An approval within `APPROVAL_GRACE_SECONDS` of
   the revision appearing (ask or edit) is refused `TooSoon` (#156, #166). Asking requests expire
   `EXPIRES_AFTER_SECONDS` after the agent last asked about them (`run_status`); a poll
   moves the expiry at most once a minute, so the others write nothing.
4. `Runner::run` reserves a `cockpit-processes` execution and marks the run running with it
   (`start_run`) in one transaction, then launches the approved revision as a terminal
   execution (#163): its own session, so prompts and `sudo` work, in a systemd scope or
   process group that terminates whole. Git hooks are off and program-running Git settings
   reset; it returns the end of the output without terminal controls and with secrets
   redacted (`present`). `start_run` and `finish_run` are fenced on the revision.
5. `Runner::reconcile` at startup, after the executions were reconciled: an approved run
   never started is `not-run`; a running one follows its execution (`reconcile_run`): never
   started is `not-run`, ended is `outcome-unknown`, still running stays `running` until
   `Runner::wait_until_ended` sees its end (`ended_after_restart`). None is repeated.

`queue_order` puts what the owner is asked in order: a running command waiting for input
first, then asking requests oldest first.

## Guides

`parse_guide` reads `# Title`, `## Section`, one step per line and indented `open:`,
`copy:`, `ask:`, `run:` lines. The owner answers the current step only (`answer_step`,
bound to its index), reports being stuck there (`report_stuck`) or closes the guide. A
`run:` step files its command as a run request of the guide's session.

## Dependency edges

- Depends on: `cockpit-turso` (transactions, migrations), `cockpit-authorization` (every
  ask is evaluated before it is filed) and `cockpit-processes` (each approved run is one of
  its executions, #163).
- Never on `cockpit-protocol`, a client, `cockpit-agent-channel` or `cockpit-projects`:
  the daemon identifies the sender, finds the project and converts to wire types
  (`cockpit-workspace-rules/tests/crate_graph.rs`).
- Used by: the daemon.

## Tests

- `tests/display.rs`, `tests/redaction.rs`: the display and redaction rules.
- `tests/hints.rs`: the `timeout` hint and the changed build files a command runs.
- `tests/runs.rs`: the run state machine on a real state database, report 04's
  stale-approval walkthrough and the restart decisions from each execution phase included.
- `tests/guides.rs`: the guide format and step answers.
- Mutations return the request as it reads after them, without reading it again; the tests
  compare the returned value with a fresh read.
- `tests/runner.rs`: a session leader that fails leaves the run not run with what it printed;
  a run a restart found launching that never started is not run.
- The pseudo-terminal runs are proven end to end in `cockpit-daemon/tests/approvals.rs`.
