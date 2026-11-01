//! `ask run`: a command an agent asks the host to run, the owner's decision on its exact
//! revision, and the run that decision authorizes.
//!
//! States: `asking` until the owner declines (`declined`) or approves (`approved`, the
//! effect intent, committed with the decision). The runner moves it to `running` with the
//! execution it reserves in `cockpit-processes` before it spawns anything, then to
//! `finished` with the exit code, or to `not-run` when nothing ran. A restart finds
//! `approved` (never spawned: `not-run`) and `running`, which its execution decides
//! ([`reconcile_run`]); the command is never repeated blindly (report 04). An asking
//! request nobody polls for [`EXPIRES_AFTER_SECONDS`] reads as expired and can no longer be
//! decided.

use std::path::{Path, PathBuf};

use cockpit_authorization::{Actor, Outcome, Permission, Scope, SessionId};
use cockpit_processes::{EndReason, ExecutionId, Phase};
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};
use sha2::{Digest, Sha256};

use crate::display::is_outside_project;
use crate::error::invalid;
use crate::pty::Output;
use crate::rows::{
    AskState, EXPIRES_AFTER_SECONDS, Project, invalid_row, next_id, not_found, project_from,
    refreshed_expiry, session_from, state_at, still_asking,
};
use crate::{Error, Refusal};

/// An approval is refused this long after the request was asked or last edited (#156,
/// #166): a keystroke meant for something else, landing as the request appears, approves
/// nothing. Clients add their own grace on top; this one holds for every client. Times are
/// whole seconds, so the grace actually lasts between one and two seconds.
pub const APPROVAL_GRACE_SECONDS: i64 = 2;
const COMMAND_MAX_BYTES: usize = 16 * 1024;
pub(crate) const REASON_MAX_CHARS: usize = 1000;
const TABLE: &str = "approvals_runs";
const KIND: &str = "run request";

/// The columns `run_from` reads, in its order.
const RUN_COLUMNS: &str = "id, session, revision, command, asked_command, cwd, project,
        project_root, reason, state, asked_at, expires_at, exit_code, output,
        output_omitted_lines, output_truncated_bytes, redacted, note, revised_at";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Asking,
    /// Asking, but nobody polled for it within [`EXPIRES_AFTER_SECONDS`]. Never stored.
    Expired,
    Declined,
    /// Approved: the run is intended and not yet started.
    Approved,
    Running,
    Finished,
    /// Approved, but nothing ran; the note says why.
    NotRun,
    /// It may have run, wholly or in part; the note says why nobody knows.
    OutcomeUnknown,
}

impl AskState for RunState {
    const ASKING: RunState = RunState::Asking;
    const EXPIRED: RunState = RunState::Expired;

    fn as_str(self) -> &'static str {
        RunState::as_str(self)
    }
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Asking => "asking",
            RunState::Expired => "expired",
            RunState::Declined => "declined",
            RunState::Approved => "approved",
            RunState::Running => "running",
            RunState::Finished => "finished",
            RunState::NotRun => "not-run",
            RunState::OutcomeUnknown => "outcome-unknown",
        }
    }

    fn parse(text: &str) -> Option<RunState> {
        [
            RunState::Asking,
            RunState::Declined,
            RunState::Approved,
            RunState::Running,
            RunState::Finished,
            RunState::NotRun,
            RunState::OutcomeUnknown,
        ]
        .into_iter()
        .find(|state| state.as_str() == text)
    }
}

/// What an agent asks to run. `cwd` is canonical (see [`resolve_cwd`]): what the owner reads
/// is where it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunAsk {
    pub command: String,
    pub cwd: PathBuf,
    pub reason: String,
    pub project: Option<Project>,
}

/// What a finished run returns to the agent: its exit code and its output, redacted,
/// without terminal controls, bounded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub exit_code: i32,
    pub output: Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRequest {
    pub id: RunId,
    pub session: SessionId,
    /// Starts at 1; every edit increments it. A decision names the revision it was made on.
    pub revision: i64,
    /// The command as it runs: the agent's, or the owner's edit of it.
    pub command: String,
    pub asked_command: String,
    pub cwd: PathBuf,
    pub project: Option<Project>,
    pub reason: String,
    pub state: RunState,
    pub asked_at: i64,
    /// When this revision appeared: the ask, or the owner's last edit.
    pub revised_at: i64,
    pub expires_at: i64,
    pub result: Option<RunResult>,
    pub note: Option<String>,
}

impl RunRequest {
    /// The digest a decision binds: this revision's working directory and command.
    pub fn digest(&self) -> String {
        action_digest(&self.cwd, &self.command)
    }

    pub fn is_outside_project(&self) -> bool {
        is_outside_project(
            &self.cwd,
            self.project.as_ref().map(|project| project.root.as_path()),
        )
    }

    pub fn was_edited(&self) -> bool {
        self.command != self.asked_command
    }
}

/// An approved run, as the runner must carry it out: exactly this revision's command in
/// exactly this directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approved {
    pub id: RunId,
    pub revision: i64,
    pub command: String,
    pub cwd: PathBuf,
}

/// SHA-256 over a versioned, length-prefixed encoding of the directory and the command, as
/// lower-case hex: no two different (directory, command) pairs encode alike.
pub fn action_digest(cwd: &Path, command: &str) -> String {
    let cwd = cwd.as_os_str().as_encoded_bytes();
    let mut hasher = Sha256::new();
    hasher.update(b"cockpit-approvals run v1\0");
    for part in [cwd, command.as_bytes()] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The directory a command would run in, resolved now: symbolic links followed, so the
/// owner reads where it runs. Refused unless it is an existing directory named absolutely.
pub fn resolve_cwd(cwd: &str) -> Result<PathBuf, Refusal> {
    let path = Path::new(cwd);
    if !path.is_absolute() {
        return Err(invalid("cwd", "must be an absolute path"));
    }
    let resolved = std::fs::canonicalize(path)
        .map_err(|error| invalid("cwd", format!("cannot be resolved: {error}")))?;
    if !resolved.is_dir() {
        return Err(invalid("cwd", "is not a directory"));
    }
    if resolved.to_str().is_none() {
        return Err(invalid("cwd", "is not UTF-8"));
    }
    Ok(resolved)
}

/// Files `ask` for the owner after `cockpit-authorization` allows `session` `run-commands`
/// over itself.
pub async fn ask_run(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    ask: RunAsk,
    now: i64,
) -> Result<RunRequest, Error> {
    validate_command("command", &ask.command)?;
    validate_reason(&ask.reason)?;
    may_ask(write, session).await?;
    insert_run(write, session, &ask, now).await
}

/// Whether `cockpit-authorization` allows `session` to ask the owner: `run-commands` over
/// itself. Every ask is evaluated so; `ask check` asks it alone.
pub async fn may_ask(write: &WriteTransaction<'_>, session: &SessionId) -> Result<(), Error> {
    let decision = cockpit_authorization::evaluate(
        write,
        &Actor::Session(session.clone()),
        Permission::RunCommands,
        &Scope::Session(session.clone()),
    )
    .await?;
    match decision.outcome {
        Outcome::Allowed(_) => Ok(()),
        Outcome::Refused(refusal) => Err(Refusal::NotAuthorized(refusal).into()),
    }
}

/// Inserts an asking request; the caller validated and authorized it.
pub(crate) async fn insert_run(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    ask: &RunAsk,
    now: i64,
) -> Result<RunRequest, Error> {
    let id = next_id(write, TABLE).await?;
    let cwd = text_of(&ask.cwd)?;
    let project_root = ask
        .project
        .as_ref()
        .map(|project| text_of(&project.root))
        .transpose()?;
    stored_run(
        write,
        &format!(
            "INSERT INTO approvals_runs (id, session, revision, command, asked_command, cwd,
                project, project_root, reason, state, asked_at, revised_at, expires_at)
             VALUES (?1, ?2, 1, ?3, ?3, ?4, ?5, ?6, ?7, 'asking', ?8, ?8, ?9)
             RETURNING {RUN_COLUMNS}"
        ),
        (
            id,
            session.as_str(),
            ask.command.as_str(),
            cwd,
            ask.project.as_ref().map(|project| project.name.clone()),
            project_root,
            ask.reason.as_str(),
            now,
            now + EXPIRES_AFTER_SECONDS,
        ),
        now,
    )
    .await
}

/// Requests the owner still has to see: asking (not expired), approved and running, oldest
/// first. [`queue_order`] puts them in the order the owner is asked.
pub async fn open_runs(connection: &Connection, now: i64) -> Result<Vec<RunRequest>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {RUN_COLUMNS} FROM approvals_runs WHERE state IN ('approved', 'running')
                    OR (state = 'asking' AND expires_at > ?1)
                 ORDER BY asked_at, id"
            ),
            [now],
        )
        .await?;
    let mut runs = Vec::new();
    while let Some(row) = rows.next().await? {
        runs.push(run_from(&row, now)?);
    }
    Ok(runs)
}

/// Report 17's queue: one approval asks at a time per owner. A running command that waits
/// for input comes to the front, then asking requests oldest first; approved commands keep
/// running behind them. `waiting_for_input` is one reading of the runner
/// (`Runner::waiting_for_input`): it changes with the clock.
pub fn queue_order(runs: &mut [RunRequest], waiting_for_input: &[RunId]) {
    runs.sort_by_key(|run| {
        let place = match run.state {
            RunState::Running if waiting_for_input.contains(&run.id) => 0,
            RunState::Asking => 1,
            _ => 2,
        };
        (place, run.asked_at, run.id)
    });
}

pub async fn run_request(
    connection: &Connection,
    id: RunId,
    now: i64,
) -> Result<Option<RunRequest>, Error> {
    let mut rows = connection
        .query(
            &format!("SELECT {RUN_COLUMNS} FROM approvals_runs WHERE id = ?1"),
            [id.0],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(run_from(&row, now)?)),
        None => Ok(None),
    }
}

/// The request as its asking session sees it. Asking about an undecided request keeps it
/// from expiring: someone still waits for the answer. Polls while the expiry is fresh
/// write nothing.
pub async fn run_status(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    id: RunId,
    now: i64,
) -> Result<RunRequest, Error> {
    let run = run_of(write, session, id, now).await?;
    match moved_expiry(&run, now) {
        Some(expires_at) => {
            stored_run(
                write,
                &format!(
                    "UPDATE approvals_runs SET expires_at = ?2 WHERE id = ?1
                     RETURNING {RUN_COLUMNS}"
                ),
                (id.0, expires_at),
                now,
            )
            .await
        }
        None => Ok(run),
    }
}

/// [`run_status`] when the poll writes nothing, read in any transaction; `None` when the
/// poll must keep the request from expiring, which only [`run_status`] may do.
pub async fn run_status_if_fresh(
    connection: &Connection,
    session: &SessionId,
    id: RunId,
    now: i64,
) -> Result<Option<RunRequest>, Error> {
    let run = run_of(connection, session, id, now).await?;
    Ok(moved_expiry(&run, now).is_none().then_some(run))
}

/// The expiry a poll of `run` at `now` stores: only a request still asking has one to move.
fn moved_expiry(run: &RunRequest, now: i64) -> Option<i64> {
    (run.state == RunState::Asking)
        .then(|| refreshed_expiry(run.expires_at, now))
        .flatten()
}

/// The request `id`, which only its asking session may see.
async fn run_of(
    connection: &Connection,
    session: &SessionId,
    id: RunId,
    now: i64,
) -> Result<RunRequest, Error> {
    match run_request(connection, id, now).await? {
        Some(run) if run.session == *session => Ok(run),
        _ => Err(not_found(KIND, id.0).into()),
    }
}

/// The owner's edit: `command` replaces the current one as a new revision, so a decision
/// made on the old revision no longer applies.
pub async fn edit_run(
    write: &WriteTransaction<'_>,
    id: RunId,
    revision: i64,
    command: &str,
    now: i64,
) -> Result<RunRequest, Error> {
    undecided_at(write, id, revision, now).await?;
    validate_command("command", command)?;
    stored_run(
        write,
        &format!(
            "UPDATE approvals_runs SET command = ?2, revision = revision + 1, revised_at = ?3
             WHERE id = ?1
             RETURNING {RUN_COLUMNS}"
        ),
        (id.0, command, now),
        now,
    )
    .await
}

pub async fn decline_run(
    write: &WriteTransaction<'_>,
    id: RunId,
    now: i64,
) -> Result<RunRequest, Error> {
    undecided(write, id, now).await?;
    stored_run(
        write,
        &format!(
            "UPDATE approvals_runs SET state = 'declined', decided_at = ?2 WHERE id = ?1
             RETURNING {RUN_COLUMNS}"
        ),
        (id.0, now),
        now,
    )
    .await
}

/// The owner's approval of exactly `revision`, whose digest must be `digest`. It is consumed
/// here: the request leaves `asking` with the run intent in the same transaction, so a second
/// approval, or one made on an older revision, is refused; so is one made within
/// [`APPROVAL_GRACE_SECONDS`] of the revision appearing. Returns the run to carry out and
/// the request as it now reads.
pub async fn approve_run(
    write: &WriteTransaction<'_>,
    id: RunId,
    revision: i64,
    digest: &str,
    now: i64,
) -> Result<(Approved, RunRequest), Error> {
    let run = undecided_at(write, id, revision, now).await?;
    let current_digest = run.digest();
    if current_digest != digest {
        return Err(Refusal::DigestMismatch.into());
    }
    let grace_ends = run.revised_at + APPROVAL_GRACE_SECONDS;
    if now < grace_ends {
        return Err(Refusal::TooSoon {
            wait_seconds: grace_ends - now,
        }
        .into());
    }
    let run = stored_run(
        write,
        &format!(
            "UPDATE approvals_runs SET state = 'approved', decided_at = ?2, approved_digest = ?3
             WHERE id = ?1
             RETURNING {RUN_COLUMNS}"
        ),
        (id.0, now, current_digest),
        now,
    )
    .await?;
    let approved = Approved {
        id,
        revision: run.revision,
        command: run.command.clone(),
        cwd: run.cwd.clone(),
    };
    Ok((approved, run))
}

/// Marks the approved run started with the execution reserved for it, in the transaction
/// that reserves it and before anything is spawned: a restart then knows it may have run,
/// and finds out from the execution what became of it.
pub async fn start_run(
    write: &WriteTransaction<'_>,
    run: &Approved,
    execution: ExecutionId,
) -> Result<(), Error> {
    fenced_update(
        write,
        run,
        "UPDATE approvals_runs SET state = 'running', execution = ?3
         WHERE id = ?1 AND revision = ?2 AND state = 'approved'",
        (run.id.0, run.revision, execution.0),
    )
    .await
}

/// Records the run's result once. Fenced on the approved revision, so no other run can
/// complete this request.
pub async fn finish_run(
    write: &WriteTransaction<'_>,
    run: &Approved,
    result: &RunResult,
    now: i64,
) -> Result<(), Error> {
    fenced_update(
        write,
        run,
        "UPDATE approvals_runs SET state = 'finished', exit_code = ?3, output = ?4,
            output_omitted_lines = ?5, output_truncated_bytes = ?6, redacted = ?7,
            ended_at = ?8
         WHERE id = ?1 AND revision = ?2 AND state = 'running'",
        (
            run.id.0,
            run.revision,
            i64::from(result.exit_code),
            result.output.shown.as_str(),
            count_column(result.output.omitted_lines),
            count_column(result.output.truncated_bytes),
            count_column(result.output.redacted),
            now,
        ),
    )
    .await
}

/// Records that the approved run did not start (its directory changed, the PTY could not be
/// opened); `note` says why.
pub async fn record_not_run(
    write: &WriteTransaction<'_>,
    run: &Approved,
    note: &str,
    now: i64,
) -> Result<(), Error> {
    fenced_update(
        write,
        run,
        "UPDATE approvals_runs SET state = 'not-run', note = ?3, ended_at = ?4
         WHERE id = ?1 AND revision = ?2 AND state IN ('approved', 'running')",
        (run.id.0, run.revision, note, now),
    )
    .await
}

/// Records that the run started but its end could not be observed or recorded: it may have
/// done anything. `note` says why.
pub async fn record_outcome_unknown(
    write: &WriteTransaction<'_>,
    run: &Approved,
    note: &str,
    now: i64,
) -> Result<(), Error> {
    fenced_update(
        write,
        run,
        "UPDATE approvals_runs SET state = 'outcome-unknown', note = ?3, ended_at = ?4
         WHERE id = ?1 AND revision = ?2 AND state = 'running'",
        (run.id.0, run.revision, note, now),
    )
    .await
}

/// A run a previous daemon left approved or running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interrupted {
    pub run: Approved,
    /// [`RunState::Approved`] or [`RunState::Running`].
    pub state: RunState,
    /// The execution the run reserved when it was marked running; `None` for an approved
    /// run, and for a running one recorded before runs had executions.
    pub execution: Option<ExecutionId>,
}

/// The runs a previous daemon left approved or running, oldest first.
pub async fn interrupted_runs(connection: &Connection) -> Result<Vec<Interrupted>, Error> {
    let mut rows = connection
        .query(
            "SELECT id, revision, command, cwd, state, execution FROM approvals_runs
             WHERE state IN ('approved', 'running') ORDER BY id",
            (),
        )
        .await?;
    let mut interrupted = Vec::new();
    while let Some(row) = rows.next().await? {
        let state: String = row.get(4)?;
        interrupted.push(Interrupted {
            run: Approved {
                id: RunId(row.get(0)?),
                revision: row.get(1)?,
                command: row.get(2)?,
                cwd: PathBuf::from(row.get::<String>(3)?),
            },
            state: if state == "running" {
                RunState::Running
            } else {
                RunState::Approved
            },
            execution: row.get::<Option<i64>>(5)?.map(ExecutionId),
        });
    }
    Ok(interrupted)
}

/// Startup, once `cockpit-processes` reconciled the executions: ends an interrupted run
/// from what its execution was observed to do, `phase` (`None`: it has none, or it is
/// unknown). A run is never started again: an execution that never started is `not-run`,
/// one that ended unobserved is `outcome-unknown`, and one still running keeps the run
/// `running` (the answer) until its end is observed.
pub async fn reconcile_run(
    write: &WriteTransaction<'_>,
    interrupted: &Interrupted,
    phase: Option<&Phase>,
    now: i64,
) -> Result<RunState, Error> {
    let run = &interrupted.run;
    if interrupted.state == RunState::Approved {
        record_not_run(
            write,
            run,
            "the daemon stopped before the approved command started",
            now,
        )
        .await?;
        return Ok(RunState::NotRun);
    }
    let note = match phase {
        Some(Phase::Launching | Phase::Running) => return Ok(RunState::Running),
        Some(Phase::Ended(EndReason::NeverStarted)) => {
            record_not_run(
                write,
                run,
                "the daemon stopped before the command started",
                now,
            )
            .await?;
            return Ok(RunState::NotRun);
        }
        Some(Phase::Ended(EndReason::HostRestarted)) => {
            "the host restarted while the command ran; what it did is unknown"
        }
        Some(Phase::Ended(EndReason::Exited | EndReason::Terminated)) => {
            "the command ended while the daemon was down; its exit code and output were not \
             observed"
        }
        None => "the daemon stopped while the command ran; what it did is unknown",
    };
    record_outcome_unknown(write, run, note, now).await?;
    Ok(RunState::OutcomeUnknown)
}

/// Runs `sql`, an `INSERT` or `UPDATE` of one row `RETURNING` [`RUN_COLUMNS`], and returns
/// that row as it was stored.
async fn stored_run(
    write: &WriteTransaction<'_>,
    sql: &str,
    params: impl cockpit_turso::sql::IntoParams,
    now: i64,
) -> Result<RunRequest, Error> {
    let row = write.prepare(sql).await?.query_row(params).await?;
    run_from(&row, now)
}

async fn fenced_update(
    write: &WriteTransaction<'_>,
    run: &Approved,
    sql: &str,
    params: impl cockpit_turso::sql::IntoParams,
) -> Result<(), Error> {
    if write.execute(sql, params).await? == 1 {
        return Ok(());
    }
    let state = run_request(write, run.id, 0)
        .await?
        .map_or("missing", |current| current.state.as_str());
    Err(Refusal::AlreadyDecided { state }.into())
}

/// The request if the owner may still decide it.
async fn undecided(write: &WriteTransaction<'_>, id: RunId, now: i64) -> Result<RunRequest, Error> {
    let run = run_request(write, id, now)
        .await?
        .ok_or_else(|| not_found(KIND, id.0))?;
    still_asking(run.state)?;
    Ok(run)
}

/// The request if the owner may still decide it and it is still at `revision`, the one the
/// owner read.
async fn undecided_at(
    write: &WriteTransaction<'_>,
    id: RunId,
    revision: i64,
    now: i64,
) -> Result<RunRequest, Error> {
    let run = undecided(write, id, now).await?;
    if run.revision != revision {
        return Err(Refusal::StaleRevision {
            current: run.revision,
        }
        .into());
    }
    Ok(run)
}

pub(crate) fn validate_command(field: &'static str, command: &str) -> Result<(), Refusal> {
    if command.trim().is_empty() {
        return Err(invalid(field, "is empty"));
    }
    if command.len() > COMMAND_MAX_BYTES {
        return Err(invalid(
            field,
            format!(
                "is {} bytes; at most {COMMAND_MAX_BYTES} (put longer work in a script file)",
                command.len()
            ),
        ));
    }
    if command.contains('\0') {
        return Err(invalid(field, "contains a NUL character"));
    }
    Ok(())
}

pub(crate) fn validate_reason(reason: &str) -> Result<(), Refusal> {
    if reason.trim().is_empty() {
        return Err(invalid(
            "reason",
            "is empty; say what the command does, why it is needed now and what to expect",
        ));
    }
    let count = reason.chars().count();
    if count > REASON_MAX_CHARS {
        return Err(invalid(
            "reason",
            format!("is {count} characters; at most {REASON_MAX_CHARS}"),
        ));
    }
    if reason.contains('\0') {
        return Err(invalid("reason", "contains a NUL character"));
    }
    Ok(())
}

fn text_of(path: &Path) -> Result<&str, Refusal> {
    path.to_str().ok_or_else(|| invalid("cwd", "is not UTF-8"))
}

/// SQLite integers are signed; counts past `i64::MAX` saturate.
fn count_column(count: u64) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

fn run_from(row: &Row, now: i64) -> Result<RunRequest, Error> {
    let stored_state = row.get::<String>(9)?;
    let stored = RunState::parse(&stored_state)
        .ok_or_else(|| invalid_row(TABLE, format!("state {stored_state:?}")))?;
    let expires_at: i64 = row.get(11)?;
    let count = |column: usize| -> Result<u64, Error> {
        let stored: i64 = row.get(column)?;
        u64::try_from(stored).map_err(|_| invalid_row(TABLE, format!("negative count {stored}")))
    };
    let result = match row.get::<Option<i64>>(12)? {
        Some(exit_code) => Some(RunResult {
            exit_code: i32::try_from(exit_code)
                .map_err(|_| invalid_row(TABLE, format!("exit code {exit_code}")))?,
            output: Output {
                shown: row.get(13)?,
                omitted_lines: count(14)?,
                truncated_bytes: count(15)?,
                redacted: count(16)?,
            },
        }),
        None => None,
    };
    Ok(RunRequest {
        id: RunId(row.get(0)?),
        session: session_from(row, 1, TABLE)?,
        revision: row.get(2)?,
        command: row.get(3)?,
        asked_command: row.get(4)?,
        cwd: PathBuf::from(row.get::<String>(5)?),
        project: project_from(row, 6, 7)?,
        reason: row.get(8)?,
        state: state_at(stored, expires_at, now),
        asked_at: row.get(10)?,
        revised_at: row.get(18)?,
        expires_at,
        result,
        note: row.get(17)?,
    })
}
