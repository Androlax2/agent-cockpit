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

use cockpit_authorization::{Actor, Basis, GrantId, Outcome, Permission, Scope, SessionId};
use cockpit_processes::{EndReason, ExecutionId, Phase};
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};
use sha2::{Digest, Sha256};

use crate::display::is_outside_project;
use crate::error::invalid;
use crate::pty::Output;
use crate::rows::{
    AskState, EXPIRES_AFTER_SECONDS, Project, invalid_row, next_id, not_found, project_from,
    refreshed_expiry, session_from, state_at, still_asking, stored_row,
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
        output_omitted_lines, output_truncated_bytes, redacted, note, revised_at, asking_grant,
        kind, start_prompt, start_clone, start_agent, start_model, start_effort, decided_at,
        decided_through, output_retired_at";
const START_PROMPT_MAX_CHARS: usize = 4000;
const START_KIND: &str = "start-session";

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
    /// Retention cleared the output long after the run (#406), or the owner purged it
    /// (#1160); `output.shown` is empty.
    pub output_retired: bool,
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
    /// The `run-commands` grant the session asked under (#285); for a start request, its
    /// `start-sessions` grant (#410).
    pub(crate) asking_grant: Option<i64>,
    /// A request to start a session (#410) rather than to run a command: the daemon starts
    /// it in [`RunRequest::cwd`] once approved, and `command` is how the owner reads it.
    pub start: Option<StartSession>,
    /// The owner's decision (#657); `None` until the request leaves `asking`.
    pub decision: Option<RunDecision>,
}

/// The owner's decision on a request: only the owner decides, so what it records is whether,
/// when and through which client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunDecision {
    pub approved: bool,
    pub at: i64,
    /// `None` when the client did not name itself, or the decision predates #657.
    pub through: Option<DecidedThrough>,
}

/// The owner client a decision came through, as that client names itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecidedThrough {
    Tui,
    Cli,
}

impl DecidedThrough {
    const ALL: [DecidedThrough; 2] = [DecidedThrough::Tui, DecidedThrough::Cli];

    pub fn as_str(self) -> &'static str {
        match self {
            DecidedThrough::Tui => "tui",
            DecidedThrough::Cli => "cli",
        }
    }

    /// How the owner reads it: `TUI`, `CLI`.
    pub fn label(self) -> &'static str {
        match self {
            DecidedThrough::Tui => "TUI",
            DecidedThrough::Cli => "CLI",
        }
    }

    fn parse(text: &str) -> Option<DecidedThrough> {
        DecidedThrough::ALL
            .into_iter()
            .find(|through| through.as_str() == text)
    }
}

/// The session a start request asks for, besides its directory, the request's `cwd` (#410).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartSession {
    /// The first message its agent gets.
    pub prompt: Option<String>,
    /// Start it in a new clone on this branch instead of in the directory itself.
    pub clone: Option<String>,
    /// The agent to run, required once the daemon wires more than one.
    pub agent: Option<String>,
    /// The model and effort, in the provider's own words (#701).
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// What a coordinator session asks to start (#410). `dir` is canonical, like
/// [`RunAsk::cwd`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartAsk {
    pub dir: PathBuf,
    pub start: StartSession,
    pub reason: String,
    pub project: Option<Project>,
}

/// Which grant a request was asked under, and so which grant must still cover its session
/// for the owner to decide it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AskedUnder {
    /// `run-commands` over the session itself: commands and guides.
    RunCommands,
    /// `start-sessions` over the project the start is in, or over everything: start
    /// requests (#410, #1131).
    StartSessions,
}

impl RunRequest {
    pub(crate) fn asked_under(&self) -> AskedUnder {
        match self.start {
            Some(_) => AskedUnder::StartSessions,
            None => AskedUnder::RunCommands,
        }
    }
}

impl RunRequest {
    /// The id of the grant the session asked under: `run-commands`, or `start-sessions` for
    /// a start request; `None` for a request filed before grants were recorded (#285).
    /// `cockpit_authorization::stored_grant` reads it.
    pub fn asking_grant(&self) -> Option<i64> {
        self.asking_grant
    }
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
    /// A start request (#410): the daemon starts this session in `cwd` instead of running
    /// `command`, which only describes it.
    pub start: Option<StartSession>,
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
    let grant = asking_grant(write, session).await?;
    insert_run(write, session, &ask, grant, now).await
}

/// Files a coordinator's request to start a session (#410) for the owner's approval in the
/// `ask run` queue. The caller evaluated `cockpit_authorization::start_allowance` for
/// `session` in this same transaction: `asked_under` is the `start-sessions` grant it
/// allowed the start under, and the request may be decided only while that grant covers the
/// session. Owner edits are refused: the owner declines, and the agent asks again.
pub async fn ask_start(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    ask: StartAsk,
    asked_under: GrantId,
    now: i64,
) -> Result<RunRequest, Error> {
    validate_reason(&ask.reason)?;
    let command = validate_start(&ask.dir, &ask.start)?;
    let run_ask = RunAsk {
        command,
        cwd: ask.dir,
        reason: ask.reason,
        project: ask.project,
    };
    let run = insert_run(write, session, &run_ask, Some(asked_under.value()), now).await?;
    stored_run(
        write,
        "UPDATE approvals_runs SET kind = ?2, start_prompt = ?3, start_clone = ?4,
             start_agent = ?5, start_model = ?6, start_effort = ?7
         WHERE id = ?1",
        (
            run.id.0,
            START_KIND,
            ask.start.prompt.clone(),
            ask.start.clone.clone(),
            ask.start.agent.clone(),
            ask.start.model.clone(),
            ask.start.effort.clone(),
        ),
        now,
    )
    .await
}

/// The checks [`ask_start`] makes on a start in `dir`, also made on a start the owner's
/// grant lets go without confirmation, so both refuse the same starts (#410). Returns how
/// the owner reads it.
pub fn validate_start(dir: &Path, start: &StartSession) -> Result<String, Refusal> {
    if let Some(prompt) = &start.prompt {
        if prompt.trim().is_empty() {
            return Err(invalid(
                "prompt",
                "is empty; leave it out to start without one",
            ));
        }
        let count = prompt.chars().count();
        if count > START_PROMPT_MAX_CHARS {
            return Err(invalid(
                "prompt",
                format!("is {count} characters; at most {START_PROMPT_MAX_CHARS}"),
            ));
        }
        if prompt.contains('\0') {
            return Err(invalid("prompt", "contains a NUL character"));
        }
    }
    // Only their shape: the provider refuses a model or effort it does not know before the ask.
    for (field, value, rule) in [
        (
            "clone",
            &start.clone,
            "must be a branch name, without spaces or control characters",
        ),
        (
            "agent",
            &start.agent,
            "must be an agent name, without spaces or control characters",
        ),
        (
            "model",
            &start.model,
            "must be one word, without spaces or control characters",
        ),
        (
            "effort",
            &start.effort,
            "must be one word, without spaces or control characters",
        ),
    ] {
        if value.as_deref().is_some_and(|value| !is_one_word(value)) {
            return Err(invalid(field, rule));
        }
    }
    let command = start_command(dir, start)?;
    validate_command("start", &command)?;
    Ok(command)
}

/// How the owner reads a start request: the `cockpit session new` that does the same, each
/// value quoted for a POSIX shell. The digest an approval binds covers it, and so every field.
fn start_command(dir: &Path, start: &StartSession) -> Result<String, Refusal> {
    let mut command = format!("torlek session new {}", shell_quoted(text_of(dir)?));
    if let Some(agent) = &start.agent {
        command.push_str(&format!(" --agent {}", shell_quoted(agent)));
    }
    if let Some(prompt) = &start.prompt {
        command.push_str(&format!(" --prompt {}", shell_quoted(prompt)));
    }
    if let Some(branch) = &start.clone {
        command.push_str(&format!(" --clone {}", shell_quoted(branch)));
    }
    if let Some(model) = &start.model {
        command.push_str(&format!(" --model {}", shell_quoted(model)));
    }
    if let Some(effort) = &start.effort {
        command.push_str(&format!(" --effort {}", shell_quoted(effort)));
    }
    Ok(command)
}

fn is_one_word(value: &str) -> bool {
    !value.is_empty()
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

fn shell_quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// Whether `cockpit-authorization` allows `session` to ask the owner: `run-commands` over
/// itself. Every ask is evaluated so; `ask check` asks it alone.
pub async fn may_ask(write: &WriteTransaction<'_>, session: &SessionId) -> Result<(), Error> {
    asking_grant(write, session).await.map(drop)
}

/// The grant `session` may ask the owner under now, which its request records; refused like
/// [`may_ask`].
pub(crate) async fn asking_grant(
    write: &WriteTransaction<'_>,
    session: &SessionId,
) -> Result<Option<i64>, Error> {
    match run_commands_over_itself(write, session).await? {
        Outcome::Allowed(basis) => Ok(grant_of(&basis)),
        Outcome::Refused(refusal) => Err(Refusal::NotAuthorized(refusal).into()),
    }
}

/// Refuses the owner's decision on a request the owner may no longer decide
/// ([`may_still_ask`]).
pub(crate) async fn asker_may_still_ask(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    asked_for: AskedUnder,
    asked_under: Option<i64>,
) -> Result<(), Error> {
    if may_still_ask(write, session, asked_for, asked_under).await? {
        Ok(())
    } else {
        Err(Refusal::AskerMayNotAsk.into())
    }
}

/// Whether the grant a request was asked under, `asked_under`, still lets `session` ask:
/// once a session ends or fails it loses `run-commands`, and nobody waits for its requests,
/// even if a restart grants it again (#285). A start request needs the very `start-sessions`
/// grant it was asked under to be active (#410): the start was checked against that grant's
/// scope (#1131), and granting it again with another scope or other terms replaces it. A
/// request recorded without a grant is dead.
pub(crate) async fn may_still_ask(
    connection: &Connection,
    session: &SessionId,
    asked_for: AskedUnder,
    asked_under: Option<i64>,
) -> Result<bool, Error> {
    if asked_under.is_none() {
        return Ok(false);
    }
    match asked_for {
        AskedUnder::RunCommands => {
            Ok(match run_commands_over_itself(connection, session).await? {
                Outcome::Allowed(basis) => grant_of(&basis) == asked_under,
                Outcome::Refused(_) => false,
            })
        }
        AskedUnder::StartSessions => Ok(cockpit_authorization::active_grants(connection, session)
            .await?
            .iter()
            .any(|grant| {
                grant.permission == Permission::StartSessions
                    && Some(grant.id.value()) == asked_under
            })),
    }
}

async fn run_commands_over_itself(
    connection: &Connection,
    session: &SessionId,
) -> Result<Outcome, Error> {
    evaluate(
        connection,
        session,
        Permission::RunCommands,
        &Scope::Session(session.clone()),
    )
    .await
}

async fn evaluate(
    connection: &Connection,
    session: &SessionId,
    permission: Permission,
    scope: &Scope,
) -> Result<Outcome, Error> {
    let decision = cockpit_authorization::evaluate(
        connection,
        &Actor::Session(session.clone()),
        permission,
        scope,
    )
    .await?;
    Ok(decision.outcome)
}

/// The grant an allowed session asks under; a session is never allowed on the owner's own
/// basis.
fn grant_of(basis: &Basis) -> Option<i64> {
    match basis {
        Basis::Grant(grant) => Some(grant.value()),
        Basis::Owner => None,
    }
}

/// Inserts an asking request; the caller validated and authorized it.
pub(crate) async fn insert_run(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    ask: &RunAsk,
    asking_grant: Option<i64>,
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
        "INSERT INTO approvals_runs (id, session, revision, command, asked_command, cwd,
                project, project_root, reason, state, asked_at, revised_at, expires_at,
                asking_grant)
             VALUES (?1, ?2, 1, ?3, ?3, ?4, ?5, ?6, ?7, 'asking', ?8, ?8, ?9, ?10)",
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
            asking_grant,
        ),
        now,
    )
    .await
}

/// Requests the owner still has to see: asking (not expired, from a session that may still
/// ask), approved and running, oldest first. [`queue_order`] puts them in the order the owner
/// is asked.
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
    let mut open = Vec::with_capacity(runs.len());
    for run in runs {
        if run.state != RunState::Asking
            || may_still_ask(
                connection,
                &run.session,
                run.asked_under(),
                run.asking_grant,
            )
            .await?
        {
            open.push(run);
        }
    }
    Ok(open)
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
    let Some(expires_at) = moved_expiry(&run, now) else {
        return Ok(run);
    };
    stored_run(
        write,
        "UPDATE approvals_runs SET expires_at = ?2 WHERE id = ?1",
        (id.0, expires_at),
        now,
    )
    .await
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

/// The request `id`, which only its asking session may see, as [`shown_run`] reads it.
async fn run_of(
    connection: &Connection,
    session: &SessionId,
    id: RunId,
    now: i64,
) -> Result<RunRequest, Error> {
    match shown_run(connection, id, now).await? {
        Some(run) if run.session == *session => Ok(run),
        _ => Err(not_found(KIND, id.0).into()),
    }
}

/// The request `id` as the owner and its asking session read it. One whose asking grant no
/// longer covers its session reads as expired, as [`open_runs`] leaves it out: a poll
/// neither keeps it alive nor leaves the agent waiting (#285), and the owner's view offers
/// no approval (#347).
pub async fn shown_run(
    connection: &Connection,
    id: RunId,
    now: i64,
) -> Result<Option<RunRequest>, Error> {
    let Some(mut run) = run_request(connection, id, now).await? else {
        return Ok(None);
    };
    if run.state == RunState::Asking
        && !may_still_ask(
            connection,
            &run.session,
            run.asked_under(),
            run.asking_grant,
        )
        .await?
    {
        run.state = RunState::Expired;
    }
    Ok(Some(run))
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
    let run = undecided_at(write, id, revision, now).await?;
    if run.start.is_some() {
        return Err(invalid(
            "command",
            "describes a start request, which is not edited: decline it, and the agent asks \
             again",
        )
        .into());
    }
    validate_command("command", command)?;
    stored_run(
        write,
        "UPDATE approvals_runs SET command = ?2, revision = revision + 1, revised_at = ?3
             WHERE id = ?1",
        (id.0, command, now),
        now,
    )
    .await
}

pub async fn decline_run(
    write: &WriteTransaction<'_>,
    through: Option<DecidedThrough>,
    id: RunId,
    now: i64,
) -> Result<RunRequest, Error> {
    undecided(write, id, now).await?;
    stored_run(
        write,
        "UPDATE approvals_runs SET state = 'declined', decided_at = ?2, decided_through = ?3
             WHERE id = ?1",
        (id.0, now, through.map(DecidedThrough::as_str)),
        now,
    )
    .await
}

/// The owner's approval of exactly `revision`, whose digest must be `digest`. It is consumed
/// here: the request leaves `asking` with the run intent in the same transaction, so a second
/// approval, or one made on an older revision, is refused; so is one made within
/// [`APPROVAL_GRACE_SECONDS`] of the revision appearing. Returns the run to carry out and
/// the request as it now reads. `through` is recorded with the decision.
pub async fn approve_run(
    write: &WriteTransaction<'_>,
    through: Option<DecidedThrough>,
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
        "UPDATE approvals_runs SET state = 'approved', decided_at = ?2, approved_digest = ?3,
                decided_through = ?4
             WHERE id = ?1",
        (
            id.0,
            now,
            current_digest,
            through.map(DecidedThrough::as_str),
        ),
        now,
    )
    .await?;
    let approved = Approved {
        id,
        revision: run.revision,
        command: run.command.clone(),
        cwd: run.cwd.clone(),
        start: run.start.clone(),
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

/// The owner's purge of one finished command run's stored output (#1160): cleared now, as
/// retention clears it once old (#406), and the rest of the request stays as the audit record.
/// A run already cleared is answered as it is.
pub async fn purge_run_output(
    write: &WriteTransaction<'_>,
    id: RunId,
    now: i64,
) -> Result<RunRequest, Error> {
    let run = run_request(write, id, now)
        .await?
        .ok_or_else(|| not_found(KIND, id.0))?;
    if run.start.is_some() {
        return Err(Refusal::NoStoredOutput {
            what: "a start request",
        }
        .into());
    }
    if run.state != RunState::Finished {
        return Err(Refusal::NoStoredOutput {
            what: run.state.as_str(),
        }
        .into());
    }
    if run
        .result
        .as_ref()
        .is_some_and(|result| result.output_retired)
    {
        return Ok(run);
    }
    stored_run(
        write,
        "UPDATE approvals_runs SET output = '', output_retired_at = ?2 WHERE id = ?1",
        (id.0, now),
        now,
    )
    .await
}

/// Records that the approved start request started `session` (#410). Fenced on the approved
/// revision, like [`finish_run`]; a start request is never `running`.
pub async fn finish_start(
    write: &WriteTransaction<'_>,
    run: &Approved,
    session: &str,
    now: i64,
) -> Result<(), Error> {
    if run.start.is_none() {
        return Err(invalid("request", "is a command, not a start request").into());
    }
    fenced_update(
        write,
        run,
        "UPDATE approvals_runs SET state = 'finished', exit_code = 0, output = ?3,
            output_omitted_lines = 0, output_truncated_bytes = 0, redacted = 0, ended_at = ?4
         WHERE id = ?1 AND revision = ?2 AND state = 'approved' AND kind = 'start-session'",
        (
            run.id.0,
            run.revision,
            format!("started session {session}"),
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
            "SELECT id, revision, command, cwd, state, execution, kind, start_prompt,
                 start_clone, start_agent, start_model, start_effort
             FROM approvals_runs
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
                start: start_from(&row, 6)?,
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

/// Runs `sql`, an `INSERT` or `UPDATE` of one request, and returns it as stored.
async fn stored_run(
    write: &WriteTransaction<'_>,
    sql: &str,
    params: impl cockpit_turso::sql::IntoParams,
    now: i64,
) -> Result<RunRequest, Error> {
    stored_row(
        write,
        &format!("{sql} RETURNING {RUN_COLUMNS}"),
        params,
        |row| run_from(row, now),
    )
    .await
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

/// The request if the owner may still approve or edit it: undecided, asked by a session
/// that may still ask, and still at `revision`, the one the owner read. A decline needs only
/// [`undecided`].
async fn undecided_at(
    write: &WriteTransaction<'_>,
    id: RunId,
    revision: i64,
    now: i64,
) -> Result<RunRequest, Error> {
    let run = undecided(write, id, now).await?;
    asker_may_still_ask(write, &run.session, run.asked_under(), run.asking_grant).await?;
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
            output_retired: row.get::<Option<i64>>(28)?.is_some(),
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
        asking_grant: row.get(19)?,
        start: start_from(row, 20)?,
        decision: decision_from(row, stored, 26)?,
    })
}

/// The decision read from `decided_at, decided_through` at `first`; `None` while asking.
/// Every state after `asking` but `declined` follows an approval.
fn decision_from(row: &Row, stored: RunState, first: usize) -> Result<Option<RunDecision>, Error> {
    let Some(at) = row.get::<Option<i64>>(first)? else {
        return Ok(None);
    };
    let through = match row.get::<Option<String>>(first + 1)? {
        Some(text) => Some(
            DecidedThrough::parse(&text)
                .ok_or_else(|| invalid_row(TABLE, format!("decided through {text:?}")))?,
        ),
        None => None,
    };
    Ok(Some(RunDecision {
        approved: stored != RunState::Declined,
        at,
        through,
    }))
}

/// The start request read from `kind, start_prompt, start_clone, start_agent, start_model,
/// start_effort` at `first`; `None` for a command.
fn start_from(row: &Row, first: usize) -> Result<Option<StartSession>, Error> {
    let kind: String = row.get(first)?;
    let prompt: Option<String> = row.get(first + 1)?;
    let clone: Option<String> = row.get(first + 2)?;
    let agent: Option<String> = row.get(first + 3)?;
    let model: Option<String> = row.get(first + 4)?;
    let effort: Option<String> = row.get(first + 5)?;
    let has_start_fields = [&prompt, &clone, &agent, &model, &effort]
        .iter()
        .any(|field| field.is_some());
    match kind.as_str() {
        START_KIND => Ok(Some(StartSession {
            prompt,
            clone,
            agent,
            model,
            effort,
        })),
        "command" if !has_start_fields => Ok(None),
        "command" => Err(invalid_row(TABLE, "a command with start fields".to_owned())),
        other => Err(invalid_row(TABLE, format!("kind {other:?}"))),
    }
}

/// Admission evidence on the caller's writer. Approved is busy before Runner reserves;
/// a finished command still names its execution, whose observed end processes owns.
pub async fn maintenance_runs(
    connection: &Connection,
) -> Result<(Option<String>, Vec<ExecutionId>), Error> {
    let mut rows = connection
        .query(
            "SELECT id,state,execution FROM approvals_runs
        WHERE state IN ('approved','running') OR execution IS NOT NULL ORDER BY id LIMIT 4097",
            (),
        )
        .await?;
    let mut executions = Vec::new();
    let mut count = 0;
    while let Some(row) = rows.next().await? {
        count += 1;
        if count > 4096 {
            return Ok((
                Some("approval execution evidence bound reached".to_owned()),
                executions,
            ));
        }
        let state: String = row.get(1)?;
        if state == "approved" || state == "running" {
            let id: i64 = row.get(0)?;
            return Ok((Some(format!("run request {id} is {state}")), executions));
        }
        if let Some(id) = row.get::<Option<i64>>(2)? {
            executions.push(ExecutionId(id));
        }
    }
    Ok((None, executions))
}

/// Caller invokes this after start_run, in the same reservation transaction.
pub(crate) async fn attribute_run_output(
    write: &WriteTransaction<'_>,
    run: &Approved,
    execution: ExecutionId,
    epoch: &str,
) -> Result<(), Error> {
    let row = write.prepare("SELECT session, project FROM approvals_runs WHERE id = ?1 AND revision = ?2 AND state = 'running'").await?
        .query_row((run.id.0, run.revision)).await?;
    let session: String = row.get(0)?;
    let project: Option<String> = row.get(1)?;
    cockpit_processes::attribute_output(
        write,
        execution,
        epoch,
        project.as_deref(),
        Some(&session),
        cockpit_processes::OutputMode::Pty,
        false,
    )
    .await?;
    Ok(())
}
