# cockpit-approvals

Owns the owner's decisions on what agents ask the host to do: each command bound to its
exact digest, working directory, revision and expiry, consumed once by the run it
authorizes, and the guides walked one step at a time.

Report 17. An agent asks with `torlek ask run` or `torlek ask guide` on the agent channel;
the daemon identifies the session (`cockpit-agent-channel`) and keeps its own copy of the
request here, so nothing is re-read from the sandbox.

## Run requests

1. `ask_run` files the request after `cockpit-authorization` allows the session
   `run-commands` over itself. The working directory is resolved first (`resolve_cwd`), and
   the shell must parse the command (`Runner::check_syntax`, `<shell> -n -c`), when the
   program the shell resolves to is known to parse without running under `-n`; any other
   shell is never given it before approval (`Syntax::NotChecked`, #494).
2. The owner reads it through `show`: hiding characters escaped as `\u{…}`, runs of blank
   lines collapsed, each kind escaped named in `Shown::warnings` (`Hidden`), non-ASCII
   characters named, a directory outside the asking agent's project flagged
   (`RunRequest::is_outside_project`). `fold` gives a long command's start and what it
   leaves out, for a pane's explicit "more". `timeout_without_foreground` and
   `build_files_run` (the changed build files and scripts a command runs, from the
   checkout's changed files the daemon reads) say what the text alone does not (#156).
   `root_runs` (`src/sudo_editable.rs`, #1383) reads the command line for what its `sudo`
   runs as root: the program, a shell's or interpreter's script, code or standard input, a
   makefile or justfile, through `cd`, `sudo -D` and the usual wrappers. Each such file the
   session can change (`SessionWrites`: its sandbox's read-write folders, or anything not
   root's alone when unsandboxed) is kept with its SHA-256, or as absent; what it cannot
   resolve is `cannot_tell`. The daemon records it with the revision (`record_root_runs`,
   column `root_runs`; an owner edit clears it until checked again), shows each file as it is
   now (`file_now`) and refuses an approved run when `changed_file` names one. The module is
   the one place the later rule scan (#1388) takes the rule from.
3. `approve_run` needs the request's current revision and its `action_digest` (SHA-256 of
   the directory and command). It consumes the request with the run intent in one
   transaction; a second decision, an older revision or another digest is refused. The
   owner's `edit_run` creates a new revision. An approval within `APPROVAL_GRACE_SECONDS` of
   the revision appearing (ask or edit) is refused `TooSoon` (#156, #166). Asking requests expire
   `EXPIRES_AFTER_SECONDS` after the agent last asked about them (`run_status`); a poll
   moves the expiry at most once a minute, so the others write nothing.
   A request records the `run-commands` grant it was asked under; once that grant no longer
   covers the session (it ended, failed or lost the grant, even if granted again since),
   `open_runs` and `open_guides` leave it out, and approving, editing or answering it is
   refused `AskerMayNotAsk`; declining or closing still works (#285).
4. `Runner::run` reserves a `cockpit-processes` execution and marks the run running with it
   (`start_run`) in one transaction, then launches the approved revision as a terminal
   execution (#163): its own session, so prompts and `sudo` work, in a systemd scope or
   process group that terminates whole. Git hooks are off and program-running Git settings
   reset; it returns the end of the output without terminal controls and with secrets
   redacted (`present`). `start_run` and `finish_run` are fenced on the revision.
   With `RunSettings::sudo` set (#1374, `src/sudo.rs`), each run's `PATH` starts with a
   `sudo` launcher that runs the trusted sudo with `-A -k`, `SUDO_ASKPASS` names the
   helper's launcher, and `sudo_asks_every_time` writes `-A -k` into a leading or
   `&&`-chained `sudo` before the owner reads the request. `Runner::sudo_helper` accepts a
   helper only when it runs the helper program with a pipe as its output and its parent has
   the trusted sudo's name and effective uid, in the helper's session, while the trusted
   file is setuid to that uid and writable by it alone. The helper then closes itself to
   tracing (`cockpit_processes::seal_against_tracing`), and `Runner::ask_sudo` asks the
   owner only when it is traced by none, no other process holds its pipe, and the session
   is a live run's. The ask waits in the run (`sudo_ask`) until
   `Runner::answer_sudo` hands it the owner's password, the run ends, or the helper goes;
   before answering it checks that the same sudo still waits on the same helper. While the
   command runs, `src/sudo_unasked.rs` reads its process tree every 10 ms for the trusted
   sudo; one that ended having asked the owner nothing through the helper is counted in
   the result's `sudo_ran_unasked`, stored in the column of that name.
   A hidden line `Runner::send_input` types, and the password `Runner::answer_sudo` hands
   on, is fingerprinted first (#1386, `src/typed_secret.rs`): the run draws a random key
   and keeps `HMAC-SHA-256(key, form)` and the length of each form (raw, base64 at three
   alignments, lower- and upper-case hex, URL encoding), then wipes the secret. The
   terminal reader scrubs each chunk before it keeps any byte, holding back the last
   `longest form - 1` bytes until the next chunk, the run's end or 1.5 s of quiet; a match
   becomes `[redacted]`, counted in the result's `typed_secret_redacted` (its column). The
   key and fingerprints are dropped with the run's end. `wipe` and `wipe_vec` overwrite
   buffers that held a secret; the daemon's frame reader uses them too.
5. `Runner::reconcile` at startup, after the executions were reconciled: an approved run
   never started is `not-run`; a running one follows its execution (`reconcile_run`): never
   started is `not-run`, ended is `outcome-unknown`, still running stays `running` until
   `Runner::wait_until_ended` sees its end (`ended_after_restart`). None is repeated.

A `start-session` request (#410) is filed by `ask_start` under the asker's `start-sessions`
grant, never through a shell: approving it hands the start back to the daemon, which starts
the session and records it with `finish_start`. It shares the queue, the revision, digest and
grace rules, and the asked-under check (`AskedUnder::StartSessions`). A restart marks an
approved start never finished `not-run`.

Retention (#406): `retirable_run_outputs` measures, and `retire_run_outputs` clears in
batches, the output of finished command runs ended before a cutoff, marking
`output_retired_at` (`RunResult::output_retired`). Every other field of the request stays as
the audit record; a start request's output is kept
(`../../docs/decisions/2026-11-04-406-database-retention.md`).

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
- `tests/sudo_editable.rs`: what a command's sudo runs that the session can edit, against
  real files, and the hashes that tell a change.
- `tests/runs.rs`: the run state machine on a real state database, report 04's
  stale-approval walkthrough and the restart decisions from each execution phase included.
- `tests/guides.rs`: the guide format and step answers.
- `tests/starts.rs`: start-session requests: asked under the grant, approved, finished (#410).
- Mutations return the request as it reads after them, without reading it again; the tests
  compare the returned value with a fresh read.
- `tests/runner.rs`: a session leader that fails leaves the run not run with what it printed;
  a run a restart found launching that never started is not run.
- The pseudo-terminal runs are proven end to end in `cockpit-daemon/tests/approvals.rs`.
