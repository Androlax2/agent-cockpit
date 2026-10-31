//! Routes `ask run`, `ask guide` and the owner's decisions to `cockpit-approvals`, and
//! converts its types to the wire at this boundary (#34). Agents ask on the agent socket,
//! where `cockpit-agent-channel` identifies the sender; the owner decides on the host-only
//! socket. The rules live in those crates; this file maps, and starts approved runs.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use cockpit_agent_channel::{PeerAncestry, ProcessError, ProcessIdentity};
use cockpit_approvals::{
    Approved, GuideId, GuideRequest, GuideState, Hidden, NoResult, Project, Refusal, RunAsk, RunId,
    RunRequest, RunResult, RunSettings, RunState, Runner, Shown,
};
use cockpit_authorization::SessionId;
use cockpit_protocol::{
    Command, ErrorCode, FoldedView, GuideStateView, GuideStepView, GuideView, HiddenView, Outcome,
    ProtocolError, Reply, RunRequestView, RunResultView, RunStateView, ShownText, Unblock,
    UnblockedBy,
};
use cockpit_turso::{Database, WriteTransaction};

use crate::routes::{Route, WorkClass};

/// The argument that makes `cockpit-daemon` the run leader of an approved command
/// (`cockpit_approvals::run_leader`) instead of the daemon.
pub const RUN_LEADER_FLAG: &str = "--run-leader";

const DEFAULT_SHELL: &str = "/bin/sh";

/// The runner approved commands go through: `$SHELL` when it is absolute, else `/bin/sh`,
/// started under this program's run leader, with `environment` (the daemon's own).
pub fn runner(environment: Vec<(OsString, OsString)>) -> std::io::Result<Runner> {
    let shell = environment
        .iter()
        .find(|(name, _)| name == "SHELL")
        .map(|(_, value)| PathBuf::from(value))
        .filter(|shell| shell.is_absolute())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SHELL));
    let mut settings = RunSettings::new(
        shell,
        std::env::current_exe()?,
        vec![RUN_LEADER_FLAG.into()],
    );
    settings.environment = environment;
    Ok(Runner::new(settings))
}

/// A request that failed, as the wire tells it.
struct Failed(ProtocolError);

type Answer = Result<Reply, Failed>;

impl From<Refusal> for Failed {
    fn from(refused: Refusal) -> Failed {
        let code = match &refused {
            Refusal::NotAuthorized(_) => ErrorCode::NotAuthorized,
            Refusal::InvalidRequest { .. } => ErrorCode::InvalidArgument,
            Refusal::NotFound { .. } => ErrorCode::NotFound,
            Refusal::StaleRevision { .. } | Refusal::DigestMismatch | Refusal::StaleStep { .. } => {
                ErrorCode::StaleRevision
            }
            Refusal::Expired | Refusal::AlreadyDecided { .. } | Refusal::NotRunning => {
                ErrorCode::Conflict
            }
            Refusal::TooSoon { .. } => ErrorCode::Refused,
        };
        let (by, action) = refused.unblock();
        Failed(ProtocolError {
            code,
            message: refused.to_string(),
            unblock: Some(Unblock {
                action,
                by: match by {
                    cockpit_approvals::UnblockedBy::Owner => UnblockedBy::Owner,
                    cockpit_approvals::UnblockedBy::Agent => UnblockedBy::Agent,
                },
            }),
        })
    }
}

impl From<cockpit_approvals::Error> for Failed {
    fn from(error: cockpit_approvals::Error) -> Failed {
        match error {
            cockpit_approvals::Error::Refused(refused) => refused.into(),
            error => internal(&error),
        }
    }
}

impl From<cockpit_agent_channel::Error> for Failed {
    fn from(error: cockpit_agent_channel::Error) -> Failed {
        match error {
            cockpit_agent_channel::Error::Refused(refused) => {
                Failed(crate::agent::refusal_to_wire(&refused))
            }
            error => internal(&error),
        }
    }
}

impl From<cockpit_turso::Error> for Failed {
    fn from(error: cockpit_turso::Error) -> Failed {
        internal(&error)
    }
}

impl From<cockpit_turso::sql::Error> for Failed {
    fn from(error: cockpit_turso::sql::Error) -> Failed {
        internal(&error)
    }
}

fn internal(error: &dyn std::fmt::Display) -> Failed {
    tracing::error!(%error, "an approvals request failed");
    Failed(ProtocolError::new(
        ErrorCode::Internal,
        "the daemon failed; the request may or may not have taken effect",
    ))
}

fn job(work: impl Future<Output = Answer> + Send + 'static) -> crate::Job {
    Box::pin(async move {
        match work.await {
            Ok(reply) => Outcome::Ok(reply),
            Err(Failed(error)) => Outcome::Error(error),
        }
    })
}

/// The route for an agent's ask; `routes.rs` sends only those here, from the agent socket.
/// Agent requests run in the bulk lane, like every agent request (#32).
pub(crate) fn route_ask(
    command: Command,
    connector: Result<ProcessIdentity, ProcessError>,
    database: Arc<Database>,
    runner: Runner,
) -> Route {
    let work = job(async move {
        match command {
            Command::AskRun {
                launch_token,
                command,
                cwd,
                reason,
            } => {
                ask_run(
                    &database,
                    &runner,
                    connector,
                    launch_token,
                    command,
                    cwd,
                    reason,
                )
                .await
            }
            Command::AskGuide {
                launch_token,
                guide,
                cwd,
            } => ask_guide(&database, &runner, connector, launch_token, &guide, &cwd).await,
            Command::AskRunStatus {
                launch_token,
                request,
            } => {
                let sender = Sender::of(connector, Cwd::Skip).await;
                let write = database.write().await?;
                let session = sender.identify(&write, launch_token).await?;
                let run =
                    cockpit_approvals::run_status(&write, &session, RunId(request), now()).await?;
                write.commit().await?;
                Ok(run_reply(&run, &runner))
            }
            Command::AskGuideStatus {
                launch_token,
                guide,
            } => {
                let sender = Sender::of(connector, Cwd::Skip).await;
                let write = database.write().await?;
                let session = sender.identify(&write, launch_token).await?;
                let guide =
                    cockpit_approvals::guide_status(&write, &session, GuideId(guide), now())
                        .await?;
                write.commit().await?;
                Ok(guide_reply(&guide))
            }
            Command::AskCheck { launch_token } => {
                let sender = Sender::of(connector, Cwd::Skip).await;
                let write = database.write().await?;
                let session = sender.identify(&write, launch_token).await?;
                cockpit_approvals::may_ask(&write, &session).await?;
                write.commit().await?;
                Ok(Reply::AskReady {
                    session: session.as_str().to_owned(),
                })
            }
            other => unreachable!("routes.rs sends only asks here, not {other:?}"),
        }
    });
    Route::Run {
        class: WorkClass::Bulk,
        job: work,
    }
}

async fn ask_run(
    database: &Database,
    runner: &Runner,
    connector: Result<ProcessIdentity, ProcessError>,
    launch_token: Option<String>,
    command: String,
    cwd: String,
    reason: String,
) -> Answer {
    let sender = Sender::of(connector, Cwd::Read).await;
    let cwd = cockpit_approvals::resolve_cwd(&cwd)?;
    check_syntax(runner, &command).await?;
    let write = database.write().await?;
    let session = sender.identify(&write, launch_token).await?;
    let project = sender.project(&write).await?;
    let ask = RunAsk {
        command,
        cwd,
        reason,
        project,
    };
    let run = cockpit_approvals::ask_run(&write, &session, ask, now()).await?;
    write.commit().await?;
    tracing::info!(
        request = run.id.0,
        session = session.as_str(),
        "an agent asked to run"
    );
    Ok(run_reply(&run, runner))
}

async fn ask_guide(
    database: &Database,
    runner: &Runner,
    connector: Result<ProcessIdentity, ProcessError>,
    launch_token: Option<String>,
    guide: &str,
    cwd: &str,
) -> Answer {
    let sender = Sender::of(connector, Cwd::Read).await;
    let guide = cockpit_approvals::parse_guide(guide)?;
    let cwd = cockpit_approvals::resolve_cwd(cwd)?;
    for command in guide.steps.iter().filter_map(|step| step.run.as_deref()) {
        check_syntax(runner, command).await?;
    }
    let write = database.write().await?;
    let session = sender.identify(&write, launch_token).await?;
    let project = sender.project(&write).await?;
    let asked = cockpit_approvals::ask_guide(&write, &session, &guide, cwd, project, now()).await?;
    write.commit().await?;
    tracing::info!(
        guide = asked.id.0,
        session = session.as_str(),
        "an agent asked a guide"
    );
    Ok(guide_reply(&asked))
}

/// Asked before any transaction: the check runs a shell, which must not hold the database.
async fn check_syntax(runner: &Runner, command: &str) -> Result<(), Failed> {
    match runner.check_syntax(command).await {
        Ok(checked) => Ok(checked?),
        Err(error) => Err(internal(&format!(
            "the shell could not check a command: {error}"
        ))),
    }
}

/// The agent at the other end of the socket, as its process tree says.
struct Sender {
    ancestry: PeerAncestry,
    /// The connecting process's working directory, which says which checkout it works in.
    cwd: Option<PathBuf>,
}

/// Whether to read the sender's working directory: a new ask needs it to find the project;
/// a poll or a check does not.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cwd {
    Read,
    Skip,
}

impl Sender {
    async fn of(connector: Result<ProcessIdentity, ProcessError>, cwd: Cwd) -> Sender {
        tokio::task::spawn_blocking(move || {
            let ancestry =
                connector.and_then(|process| cockpit_agent_channel::ancestry_of(&process));
            // Read after the ancestry verified the pid's start time; only the display's
            // "outside the project" flag depends on it.
            let cwd = match (cwd, &ancestry) {
                (Cwd::Read, Ok(chain)) => chain
                    .first()
                    .and_then(|leaf| std::fs::read_link(format!("/proc/{}/cwd", leaf.pid)).ok()),
                _ => None,
            };
            Sender { ancestry, cwd }
        })
        .await
        .expect("reading the process table does not panic")
    }

    async fn identify(
        &self,
        write: &WriteTransaction<'_>,
        launch_token: Option<String>,
    ) -> Result<SessionId, Failed> {
        Ok(cockpit_agent_channel::identify(write, &self.ancestry, launch_token.as_deref()).await?)
    }

    /// The registered checkout the sender works in, deepest first. Until sessions record
    /// their checkout (#31), the sender's working directory is the evidence.
    async fn project(&self, write: &WriteTransaction<'_>) -> Result<Option<Project>, Failed> {
        let Some(cwd) = &self.cwd else {
            return Ok(None);
        };
        let checkouts = cockpit_projects::checkouts(write, None)
            .await
            .map_err(|error| internal(&error))?;
        Ok(checkouts
            .into_iter()
            .filter(|checkout| cwd.starts_with(&checkout.path))
            .max_by_key(|checkout| checkout.path.components().count())
            .map(|checkout| Project {
                name: checkout.project.as_str().to_owned(),
                root: checkout.path,
            }))
    }
}

/// The route for one of the owner's decisions; `routes.rs` sends only those here, from the
/// host-only socket. Decisions are small and latency-bound: the control lane.
pub(crate) fn route_decision(command: Command, database: Arc<Database>, runner: Runner) -> Route {
    let work = job(async move {
        match command {
            Command::ApprovalQueue => {
                let read = database.read().await?;
                let mut runs = cockpit_approvals::open_runs(&read, now()).await?;
                let guides = cockpit_approvals::open_guides(&read, now()).await?;
                // Git runs below; the database is not held meanwhile.
                drop(read);
                let waiting = runner.waiting_for_input();
                cockpit_approvals::queue_order(&mut runs, &waiting);
                let mut changes = CheckoutChanges::default();
                let mut views = Vec::with_capacity(runs.len());
                for run in &runs {
                    views.push(run_view(run, &waiting, changes.build_files_run(run).await));
                }
                Ok(Reply::ApprovalQueue {
                    runs: views,
                    guides: guides.iter().map(guide_view).collect(),
                })
            }
            Command::ApprovalShow { request } => {
                let read = database.read().await?;
                let run = existing_run(&read, request).await?;
                drop(read);
                Ok(owner_run_reply(&run, &runner).await)
            }
            Command::ApprovalApprove {
                request,
                revision,
                digest,
            } => approve(database, runner, request, revision, &digest).await,
            Command::ApprovalDecline { request } => {
                let write = database.write().await?;
                let run = cockpit_approvals::decline_run(&write, RunId(request), now()).await?;
                write.commit().await?;
                tracing::info!(request, "the owner declined a run");
                Ok(run_reply(&run, &runner))
            }
            Command::ApprovalEdit {
                request,
                revision,
                command,
            } => {
                check_syntax(&runner, &command).await?;
                let write = database.write().await?;
                let run =
                    cockpit_approvals::edit_run(&write, RunId(request), revision, &command, now())
                        .await?;
                write.commit().await?;
                tracing::info!(request, revision = run.revision, "the owner edited a run");
                Ok(owner_run_reply(&run, &runner).await)
            }
            Command::ApprovalInput { request, text } => {
                runner.send_input(RunId(request), &text)?;
                let read = database.read().await?;
                Ok(run_reply(&existing_run(&read, request).await?, &runner))
            }
            Command::GuideShow { guide } => {
                let read = database.read().await?;
                let found = cockpit_approvals::guide_request(&read, GuideId(guide), now())
                    .await?
                    .ok_or(Refusal::NotFound {
                        kind: "guide",
                        id: guide,
                    })?;
                Ok(guide_reply(&found))
            }
            Command::GuideAnswer {
                guide,
                step,
                answer,
            } => {
                let write = database.write().await?;
                let answered =
                    cockpit_approvals::answer_step(&write, GuideId(guide), step, answer, now())
                        .await?;
                write.commit().await?;
                Ok(guide_reply(&answered))
            }
            Command::GuideStuck { guide, step, note } => {
                let write = database.write().await?;
                let stuck =
                    cockpit_approvals::report_stuck(&write, GuideId(guide), step, &note, now())
                        .await?;
                write.commit().await?;
                Ok(guide_reply(&stuck))
            }
            Command::GuideClose { guide } => {
                let write = database.write().await?;
                let closed = cockpit_approvals::close_guide(&write, GuideId(guide), now()).await?;
                write.commit().await?;
                Ok(guide_reply(&closed))
            }
            other => unreachable!("routes.rs sends only decisions here, not {other:?}"),
        }
    });
    Route::Run {
        class: WorkClass::Control,
        job: work,
    }
}

async fn existing_run(
    connection: &cockpit_turso::sql::Connection,
    request: i64,
) -> Result<RunRequest, Failed> {
    Ok(
        cockpit_approvals::run_request(connection, RunId(request), now())
            .await?
            .ok_or(Refusal::NotFound {
                kind: "run request",
                id: request,
            })?,
    )
}

/// Consumes the request with the run intent, then starts the run. The run is not the
/// request's work: it outlives the reply, and a long command must not hold a control
/// worker. A restart before it starts leaves it `not-run`, never repeated.
async fn approve(
    database: Arc<Database>,
    runner: Runner,
    request: i64,
    revision: i64,
    digest: &str,
) -> Answer {
    let write = database.write().await?;
    let (approved, run) =
        cockpit_approvals::approve_run(&write, RunId(request), revision, digest, now()).await?;
    write.commit().await?;
    tracing::info!(request, revision, "the owner approved a run");
    let reply = run_reply(&run, &runner);
    tokio::spawn(execute(database, runner, approved));
    Ok(reply)
}

async fn execute(database: Arc<Database>, runner: Runner, approved: Approved) {
    let request = approved.id.0;
    if let Err(Failed(error)) = start(&database, &approved).await {
        tracing::error!(request, ?error, "an approved run could not start");
        let ended = Err(NoResult::NotStarted(format!(
            "the run could not be marked running: {}",
            error.message
        )));
        record_end_or_log(&database, &approved, &ended).await;
        return;
    }
    let ended = match safe_git_config(&approved.cwd).await {
        Ok(git_config) => runner.run(&approved, &git_config).await,
        Err(reason) => Err(NoResult::NotStarted(reason)),
    };
    match &ended {
        Ok(ran) => {
            if ran.terminal_held_open {
                tracing::warn!(
                    request,
                    "a background process kept the terminal open; its later output is lost"
                );
            }
            tracing::info!(
                request,
                exit_code = ran.result.exit_code,
                "an approved run finished"
            );
        }
        Err(no_result) => tracing::warn!(request, ?no_result, "an approved run has no result"),
    }
    let ended = ended.map(|ran| ran.result);
    record_end_or_log(&database, &approved, &ended).await;
}

/// `cockpit-projects`' safe Git settings for a run in `cwd`, read just before it starts: the
/// repository's own program-running drivers are reset too.
async fn safe_git_config(cwd: &Path) -> Result<Vec<String>, String> {
    let cwd = cwd.to_path_buf();
    tokio::task::spawn_blocking(move || cockpit_projects::Git::from_path().safe_config(&cwd))
        .await
        .map_err(|error| format!("the Git configuration check stopped: {error}"))?
        .map_err(|error| format!("cannot reset the repository's Git drivers: {error}"))
}

async fn start(database: &Database, approved: &Approved) -> Result<(), Failed> {
    let write = database.write().await?;
    cockpit_approvals::start_run(&write, approved).await?;
    write.commit().await?;
    Ok(())
}

/// Records the run's end. When that fails after the command started, the run is marked
/// `outcome-unknown` rather than left running for ever; a daemon restart reconciles what
/// even that cannot record.
async fn record_end_or_log(
    database: &Database,
    approved: &Approved,
    ended: &Result<RunResult, NoResult>,
) {
    let request = approved.id.0;
    let Err(Failed(error)) = record_end(database, approved, ended).await else {
        return;
    };
    tracing::error!(request, ?error, "an approved run's end was not recorded");
    if matches!(ended, Err(NoResult::NotStarted(_))) {
        return;
    }
    let ended = Err(NoResult::Unobserved(format!(
        "the run's end could not be recorded: {}",
        error.message
    )));
    if let Err(Failed(error)) = record_end(database, approved, &ended).await {
        tracing::error!(
            request,
            ?error,
            "an approved run is left running until restart"
        );
    }
}

async fn record_end(
    database: &Database,
    approved: &Approved,
    ended: &Result<RunResult, NoResult>,
) -> Result<(), Failed> {
    let write = database.write().await?;
    match ended {
        Ok(result) => cockpit_approvals::finish_run(&write, approved, result, now()).await?,
        Err(NoResult::NotStarted(reason)) => {
            cockpit_approvals::record_not_run(&write, approved, reason, now()).await?;
        }
        Err(NoResult::Unobserved(reason)) => {
            cockpit_approvals::record_outcome_unknown(&write, approved, reason, now()).await?;
        }
    }
    write.commit().await?;
    Ok(())
}

pub(crate) fn now() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970");
    i64::try_from(elapsed.as_secs()).expect("seconds since 1970 fit in an i64")
}

/// A run request as its asking agent reads it.
fn run_reply(run: &RunRequest, runner: &Runner) -> Reply {
    Reply::RunRequest {
        request: Box::new(run_view(run, &runner.waiting_for_input(), None)),
    }
}

/// A run request as the owner reads it before deciding: with the changed build files it
/// would run, read now.
async fn owner_run_reply(run: &RunRequest, runner: &Runner) -> Reply {
    let changed = CheckoutChanges::default().build_files_run(run).await;
    Reply::RunRequest {
        request: Box::new(run_view(run, &runner.waiting_for_input(), changed)),
    }
}

/// The changed files of each checkout the owner's view reads, each read once per view: a
/// queue of requests from one project runs Git once.
#[derive(Default)]
struct CheckoutChanges {
    read: HashMap<PathBuf, Option<Vec<String>>>,
}

impl CheckoutChanges {
    /// The build files and scripts `run` would run that work in its project's checkout
    /// changed (`Git::changed_files`, #156). Read when the owner looks, not when the agent
    /// asked: a file changed after the ask still shows. `None` when the request is decided,
    /// its project is unknown, or Git could not read the checkout; the owner's view says so.
    async fn build_files_run(&mut self, run: &RunRequest) -> Option<Vec<String>> {
        if run.state != RunState::Asking {
            return None;
        }
        let root = &run.project.as_ref()?.root;
        let changed = match self.read.get(root) {
            Some(changed) => changed,
            None => {
                let changed = changed_files(root.clone()).await;
                self.read.entry(root.clone()).or_insert(changed)
            }
        };
        Some(cockpit_approvals::build_files_run(
            &run.command,
            changed.as_deref()?,
        ))
    }
}

async fn changed_files(root: PathBuf) -> Option<Vec<String>> {
    let read = tokio::task::spawn_blocking(move || {
        cockpit_projects::Git::from_path().changed_files(&root)
    })
    .await
    .expect("reading a checkout's changes does not panic");
    // Debug, not warn: the owner's views poll, and each already shows the read failed.
    read.inspect_err(|error| tracing::debug!(%error, "cannot read a checkout's changed files"))
        .ok()
}

fn guide_reply(guide: &GuideRequest) -> Reply {
    Reply::Guide {
        guide: Box::new(guide_view(guide)),
    }
}

fn shown(text: &str) -> ShownText {
    let Shown {
        text,
        has_hidden_characters,
        non_ascii,
        warnings,
    } = cockpit_approvals::show(text);
    ShownText {
        text,
        has_hidden_characters,
        non_ascii,
        warnings: warnings
            .into_iter()
            .map(|kind| match kind {
                Hidden::BidiControl => HiddenView::BidiControl,
                Hidden::Invisible => HiddenView::Invisible,
                Hidden::Control => HiddenView::Control,
                Hidden::BlankLines => HiddenView::BlankLines,
            })
            .collect(),
    }
}

/// `waiting_for_input`: the runs the runner sees waiting at a prompt. `changed_build_files`:
/// see [`CheckoutChanges::build_files_run`]; agents' views leave it out.
fn run_view(
    run: &RunRequest,
    waiting_for_input: &[RunId],
    changed_build_files: Option<Vec<String>>,
) -> RunRequestView {
    let cwd = crate::projects::text(&run.cwd);
    let shown_command = shown(&run.command);
    RunRequestView {
        id: run.id.0,
        session: run.session.as_str().to_owned(),
        revision: run.revision,
        state: match run.state {
            RunState::Asking => RunStateView::Asking,
            RunState::Expired => RunStateView::Expired,
            RunState::Declined => RunStateView::Declined,
            RunState::Approved => RunStateView::Approved,
            RunState::Running => RunStateView::Running,
            RunState::Finished => RunStateView::Finished,
            RunState::NotRun => RunStateView::NotRun,
            RunState::OutcomeUnknown => RunStateView::OutcomeUnknown,
        },
        command: run.command.clone(),
        asked_command: run.was_edited().then(|| run.asked_command.clone()),
        folded_command: cockpit_approvals::fold(&shown_command.text).map(|folded| FoldedView {
            preview: folded.preview,
            more_lines: folded.more_lines,
            more_chars: folded.more_chars,
        }),
        timeout_without_foreground: cockpit_approvals::timeout_without_foreground(&run.command),
        changed_build_files,
        shown_command,
        shown_cwd: shown(&cwd),
        shown_reason: shown(&run.reason),
        cwd,
        reason: run.reason.clone(),
        project: run.project.as_ref().map(|project| project.name.clone()),
        outside_project: run.is_outside_project(),
        digest: run.digest(),
        asked_at: run.asked_at,
        expires_at: run.expires_at,
        waiting_for_input: run.state == RunState::Running && waiting_for_input.contains(&run.id),
        result: run.result.as_ref().map(|result| RunResultView {
            exit_code: result.exit_code,
            output: result.output.shown.clone(),
            omitted_lines: result.output.omitted_lines,
            truncated_bytes: result.output.truncated_bytes,
            redacted: result.output.redacted,
        }),
        note: run.note.clone(),
    }
}

fn guide_view(guide: &GuideRequest) -> GuideView {
    let cwd = crate::projects::text(&guide.cwd);
    GuideView {
        id: guide.id.0,
        session: guide.session.as_str().to_owned(),
        title: shown(&guide.title),
        shown_cwd: shown(&cwd),
        cwd,
        project: guide.project.as_ref().map(|project| project.name.clone()),
        outside_project: guide.is_outside_project(),
        state: match guide.state {
            GuideState::Asking => GuideStateView::Asking,
            GuideState::Expired => GuideStateView::Expired,
            GuideState::Done => GuideStateView::Done,
            GuideState::Stuck => GuideStateView::Stuck,
            GuideState::Closed => GuideStateView::Closed,
        },
        current_step: guide.current_step,
        steps: guide
            .steps
            .iter()
            .map(|step| GuideStepView {
                section: step.step.section.as_deref().map(shown),
                text: shown(&step.step.text),
                open: step.step.open.iter().map(|value| shown(value)).collect(),
                copy: step.step.copy.iter().map(|value| shown(value)).collect(),
                ask: step.step.ask.as_deref().map(shown),
                run: step.step.run.as_deref().map(shown),
                answer: step.answer.clone(),
                run_request: step.run_request.map(|run| run.0),
            })
            .collect(),
        stuck_note: guide.stuck_note.clone(),
        asked_at: guide.asked_at,
        expires_at: guide.expires_at,
    }
}
