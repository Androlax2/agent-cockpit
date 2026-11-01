//! Routes `cockpit check` to `cockpit-verification` and converts its types to the wire at this
//! boundary (#313). A receipt is recorded only for a run this daemon executed: its run id is
//! the execution, its artifacts are fingerprinted before the run starts, and its source comes
//! from the daemon's own Git call, never from a payload. An agent's statement that a check
//! passed is a claim, shown and never counted. A gate's evidence is loaded on a read
//! transaction and evaluated after it ends, because evaluation reads the checkout.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;

use cockpit_agent_channel::{ProcessError, ProcessIdentity};
use cockpit_approvals::RunSettings;
use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_processes::{EndReason, ExecutionId, LaunchSpec, OutputKind, Phase, Supervisor};
use cockpit_protocol::{
    CheckArtifactView, CheckClaimView, CheckOutcomeView, CheckReceiptView, CheckStatusView,
    Command, ErrorCode, GateCheckView, GateOutcomeView, GateView, ProtocolError, Reply,
    StaleReasonView, UnblockedBy,
};
use cockpit_turso::Database;
use cockpit_turso::sql::Connection;
use cockpit_verification::{
    CheckDefinition, CheckStatus, Claim, GateOutcome, GateReport, NewClaim, NewReceipt, Outcome,
    Receipt, Refusal, Source, StaleReason,
};
use tokio::sync::oneshot;

use crate::approvals::{Answer, Failed, Sender, job, now};
use crate::routes::{Peer, Route, WorkClass};
use crate::tasks::refused;

/// The one environment the daemon runs checks in so far: on the host, as the owner's
/// approved commands run.
const NATIVE: &str = "native";

/// A check's command is POSIX shell whatever the owner's login shell is, so a project's
/// checks read the same on every host.
const CHECK_SHELL: &str = "/bin/sh";

/// What a verification command needs: the state, the host Git that reads a checkout's source,
/// the supervisor that runs checks, how approved commands run (the shell and environment a
/// check gets too), and the owner's environment for the project's configuration.
#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) database: Arc<Database>,
    pub(crate) git: cockpit_projects::Git,
    pub(crate) processes: Supervisor,
    pub(crate) run_settings: Arc<RunSettings>,
    pub(crate) environment: cockpit_config::Environment,
}

/// A registered checkout of a project.
struct Target {
    project: String,
    path: PathBuf,
}

/// The route for a verification command; `routes.rs` sends each only from the socket it is
/// for. Every one reads the checkout (Git hashes the worktree), so all run in the bulk lane.
pub(crate) fn route(command: Command, peer: Peer, context: Context) -> Route {
    let work = async move {
        match (command, peer) {
            (
                Command::CheckRun {
                    project,
                    check,
                    checkout,
                },
                Peer::Owner,
            ) => {
                let checkout = owner_checkout(&context, &project, checkout.as_deref()).await?;
                require_trusted(&context, &checkout).await?;
                run(&context, target(checkout), &check).await
            }
            (Command::CheckReceipt { project, run }, Peer::Owner) => {
                receipt(&context, &project, &run).await
            }
            (
                Command::CheckGate {
                    project: Some(project),
                    checkout,
                    ..
                },
                Peer::Owner,
            ) => {
                let checkout = owner_checkout(&context, &project, checkout.as_deref()).await?;
                gate(&context, target(checkout)).await
            }
            (Command::CheckGate { project: None, .. }, Peer::Owner) => Err(refused(
                ErrorCode::InvalidArgument,
                "name the project whose gate to report".to_owned(),
                "pass the project's name",
                UnblockedBy::Owner,
            )),
            (
                Command::CheckGate {
                    launch_token,
                    project,
                    checkout,
                },
                Peer::Agent { connector },
            ) => {
                if project.is_some() || checkout.is_some() {
                    return Err(refused(
                        ErrorCode::InvalidArgument,
                        "an agent's gate is the checkout it works in; it names none".to_owned(),
                        "ask without a project or checkout",
                        UnblockedBy::Agent,
                    ));
                }
                let agent = Agent::identified(&context, connector, launch_token).await?;
                gate(&context, agent.target).await
            }
            (
                Command::CheckClaim {
                    launch_token,
                    check,
                    statement,
                },
                Peer::Agent { connector },
            ) => claim(&context, connector, launch_token, check, statement).await,
            (command, _) => unreachable!(
                "routes.rs sends only verification commands, each from its socket: {command:?}"
            ),
        }
    };
    Route::Run {
        class: WorkClass::Bulk,
        job: job(work),
    }
}

/// The project's registered checkout at `checkout` (its root when absent).
async fn owner_checkout(
    context: &Context,
    project: &str,
    checkout: Option<&str>,
) -> Result<cockpit_projects::Checkout, Failed> {
    let read = context.database.read().await?;
    let found = match cockpit_projects::project_named(&read, project).await {
        Ok(found) => found,
        Err(cockpit_projects::Error::UnknownProject { .. }) => {
            return Err(refused(
                ErrorCode::NotFound,
                format!("project {project} is not registered"),
                "register it with `cockpit project add`",
                UnblockedBy::Owner,
            ));
        }
        Err(error) => return Err(internal(&error)),
    };
    let wanted = checkout.map_or_else(|| found.root.clone(), PathBuf::from);
    cockpit_projects::checkouts(&read, Some(&found.name))
        .await
        .map_err(|error| internal(&error))?
        .into_iter()
        .find(|known| known.path == wanted)
        .ok_or_else(|| {
            refused(
                ErrorCode::NotFound,
                format!(
                    "{} is not a registered checkout of {project}",
                    wanted.display()
                ),
                "name a path `cockpit checkout list` shows",
                UnblockedBy::Owner,
            )
        })
}

fn target(checkout: cockpit_projects::Checkout) -> Target {
    Target {
        project: checkout.project.as_str().to_owned(),
        path: checkout.path,
    }
}

/// A check runs the command the checkout's own configuration declares, on the host: only in
/// a checkout the owner trusts, never in a review checkout (report 09, report 11).
async fn require_trusted(
    context: &Context,
    checkout: &cockpit_projects::Checkout,
) -> Result<(), Failed> {
    let trusted = crate::projects::trusted_roots(context.environment.clone())
        .await
        .map_err(|error| {
            refused(
                ErrorCode::Refused,
                error.to_string(),
                "fix the owner configuration",
                UnblockedBy::Owner,
            )
        })?;
    match cockpit_projects::trust(checkout, &trusted) {
        cockpit_projects::Trust::Trusted | cockpit_projects::Trust::Inherited { .. } => Ok(()),
        cockpit_projects::Trust::Untrusted | cockpit_projects::Trust::NeverForReview => {
            Err(refused(
                ErrorCode::NotAuthorized,
                format!(
                    "{} is not trusted: its checks are commands its own files declare",
                    checkout.path.display()
                ),
                "trust the project under `projects.trusted` in the owner configuration",
                UnblockedBy::Owner,
            ))
        }
    }
}

/// The agent at the other end of the socket: its session and the checkout it works in.
struct Agent {
    session: SessionId,
    target: Target,
}

impl Agent {
    /// Identifies the sender on a read transaction when its launch is bound, else under the
    /// writer, and finds the registered checkout it works in.
    async fn identified(
        context: &Context,
        connector: Result<ProcessIdentity, ProcessError>,
        launch_token: Option<String>,
    ) -> Result<Agent, Failed> {
        let sender =
            Sender::once_launched(connector, &context.database, launch_token.as_deref()).await?;
        let read = context.database.read().await?;
        let bound = sender
            .identify_bound(&read, launch_token.as_deref())
            .await?;
        let project = sender.project(&read).await?;
        drop(read);
        let session = match bound {
            Some(session) => session,
            None => {
                let write = context.database.write().await?;
                let session = sender.identify(&write, launch_token).await?;
                write.commit().await?;
                session
            }
        };
        let project = project.ok_or_else(|| {
            refused(
                ErrorCode::NotFound,
                "the session does not work in a registered checkout".to_owned(),
                "work in a checkout `cockpit project add` registered",
                UnblockedBy::Owner,
            )
        })?;
        Ok(Agent {
            session,
            target: Target {
                project: project.name,
                path: project.root,
            },
        })
    }
}

/// The project's declared checks, read from the checkout's configuration and the owner's.
async fn declared_checks(
    context: &Context,
    checkout: &Path,
) -> Result<Vec<cockpit_config::Check>, Failed> {
    let environment = context.environment.clone();
    let checkout = checkout.to_path_buf();
    let loaded = tokio::task::spawn_blocking(move || {
        cockpit_config::load(&environment, Some(&checkout), &[])
    })
    .await
    .map_err(|error| internal(&error))?;
    let config = loaded.map_err(|error| {
        refused(
            ErrorCode::Refused,
            format!("the project configuration cannot be read: {error}"),
            "fix the configuration file",
            UnblockedBy::Owner,
        )
    })?;
    Ok(config.checks.into_iter().map(|check| check.value).collect())
}

fn definition(check: &cockpit_config::Check) -> CheckDefinition {
    CheckDefinition {
        name: check.name.clone(),
        command: check.command.clone(),
        working_directory: check.working_directory.clone(),
        environment: check.environment.clone(),
    }
}

fn declared<'checks>(
    checks: &'checks [cockpit_config::Check],
    name: &str,
    project: &str,
) -> Result<&'checks cockpit_config::Check, Failed> {
    checks
        .iter()
        .find(|check| check.name == name)
        .ok_or_else(|| {
            refused(
                ErrorCode::NotFound,
                format!("project {project} declares no check `{name}`"),
                "declare it under `[[checks]]` in the project's .cockpit/config.toml",
                UnblockedBy::Owner,
            )
        })
}

/// The checkout's head and uncommitted changes, read by the daemon's own Git.
async fn source(context: &Context, checkout: &Path) -> Result<Source, Failed> {
    let git = context.git.clone();
    let checkout = checkout.to_path_buf();
    let state = tokio::task::spawn_blocking(move || git.source_state(&checkout))
        .await
        .map_err(|error| internal(&error))?
        .map_err(|error| {
            refused(
                ErrorCode::Refused,
                format!("the checkout's source cannot be read: {error}"),
                "commit at least once, and make the checkout one Git can read",
                UnblockedBy::Owner,
            )
        })?;
    Ok(Source {
        head: state.head,
        base: None,
        dirty: state.dirty,
    })
}

/// Starts `check` in `target` and answers with its run; the receipt is recorded when it ends.
async fn run(context: &Context, target: Target, check: &str) -> Answer {
    let checks = declared_checks(context, &target.path).await?;
    let check = declared(&checks, check, &target.project)?.clone();
    let definition = definition(&check);
    definition.validate()?;
    if definition.environment != NATIVE {
        return Err(refused(
            ErrorCode::Refused,
            format!(
                "check `{}` runs in `{}`; this daemon runs checks only `{NATIVE}`",
                definition.name, definition.environment
            ),
            "declare the check's environment `native`",
            UnblockedBy::Owner,
        ));
    }
    let source = source(context, &target.path).await?;
    let (artifacts, git_config) = {
        let git = context.git.clone();
        let checkout = target.path.clone();
        let paths = check.artifacts.clone();
        tokio::task::spawn_blocking(move || {
            let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
            let artifacts = cockpit_verification::fingerprint(&checkout, &paths);
            (artifacts, git.safe_config(&checkout))
        })
        .await
        .map_err(|error| internal(&error))?
    };
    let artifacts = artifacts?;
    let git_config = git_config.map_err(|error| internal(&error))?;
    let write = context.database.write().await?;
    let execution = context
        .processes
        .reserve(&write, context.database.epoch())
        .await
        .map_err(|error| internal(&error))?;
    write.commit().await?;
    let spec = LaunchSpec {
        program: CHECK_SHELL.into(),
        // One stream, in the order it was written: failures print last.
        arguments: vec![
            "-c".into(),
            format!("exec 2>&1\n{}", definition.command).into(),
        ],
        working_dir: target.path.join(&definition.working_directory),
        environment: context.run_settings.run_environment(&git_config),
        terminal: None,
    };
    let run_id = format!("execution-{}", execution.0);
    let started_at = now();
    let launched = context
        .processes
        .launch_reserved(&context.database, execution, &spec)
        .await;
    let mut pending = NewReceipt {
        project: target.project,
        run_id: run_id.clone(),
        check: definition,
        source,
        artifacts,
        outcome: Outcome::Unknown,
        exit_code: None,
        output: String::new(),
        output_references: Vec::new(),
        started_at,
        finished_at: started_at,
    };
    match launched {
        Ok(launched) => {
            tokio::spawn(record_when_ended(
                context.clone(),
                execution,
                launched.exit,
                pending,
            ));
        }
        Err(error) if never_started(context, execution).await => {
            return Err(refused(
                ErrorCode::Refused,
                format!("check `{}` could not start: {error}", pending.check.name),
                "make the check's working directory exist",
                UnblockedBy::Owner,
            ));
        }
        // A launch whose confirmation timed out may still run: its outcome is unknown, and
        // the gate asks for the check again.
        Err(error) => {
            pending.output = format!("cockpit lost the run: the check may have started: {error}");
            let output_bytes = pending.output.len() as u64;
            record(context, pending, output_bytes).await;
        }
    }
    Ok(Reply::CheckRunStarted { run: run_id })
}

/// Waits for the check's program to exit, then records what it found with the end of its
/// output. A daemon that stops first records nothing for the run.
async fn record_when_ended(
    context: Context,
    execution: ExecutionId,
    exit: oneshot::Receiver<std::io::Result<ExitStatus>>,
    mut receipt: NewReceipt,
) {
    (receipt.outcome, receipt.exit_code) = match exit.await {
        Ok(Ok(status)) => match status.code() {
            Some(0) => (Outcome::Passed, Some(0)),
            Some(code) => (Outcome::Failed, Some(i64::from(code))),
            // Ended by a signal: it did not pass.
            None => (Outcome::Failed, None),
        },
        Ok(Err(_)) | Err(_) => (Outcome::Unknown, None),
    };
    receipt.finished_at = now();
    let output_path =
        context
            .processes
            .output_path(&context.database, execution, OutputKind::Stdout);
    receipt.output_references = vec![output_path.display().to_string()];
    let read = tokio::task::spawn_blocking(move || output_tail(&output_path)).await;
    let output_bytes = match read {
        Ok(Ok((tail, bytes))) => {
            receipt.output = tail;
            bytes
        }
        Ok(Err(error)) => {
            receipt.output = format!("cockpit could not read the output: {error}");
            receipt.output.len() as u64
        }
        Err(error) => {
            receipt.output = format!("cockpit could not read the output: {error}");
            receipt.output.len() as u64
        }
    };
    record(&context, receipt, output_bytes).await;
}

async fn never_started(context: &Context, execution: ExecutionId) -> bool {
    matches!(
        cockpit_processes::execution(&context.database, execution).await,
        Ok(observed) if observed.phase == Phase::Ended(EndReason::NeverStarted)
    )
}

/// Records the receipt, logging the outcome: the run already ended, so a failure here can
/// only be told in the daemon's log.
async fn record(context: &Context, receipt: NewReceipt, output_bytes: u64) {
    let run_id = receipt.run_id.clone();
    let recorded = async {
        let write = context.database.write().await?;
        let receipt =
            cockpit_verification::record_receipt_of_tail(&write, receipt, output_bytes, now())
                .await
                .map_err(|error| error.to_string())?;
        write.commit().await?;
        Ok::<Receipt, Box<dyn std::error::Error + Send + Sync>>(receipt)
    };
    match recorded.await {
        Ok(receipt) => tracing::info!(
            run = run_id,
            receipt = receipt.id.0,
            outcome = receipt.recorded.outcome.as_str(),
            "recorded a check run"
        ),
        Err(error) => tracing::error!(run = run_id, %error, "a check run could not be recorded"),
    }
}

/// The last [`cockpit_verification::OUTPUT_MAX_BYTES`] of the file and its full size.
fn output_tail(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let kept = cockpit_verification::OUTPUT_MAX_BYTES as u64;
    file.seek(SeekFrom::Start(size.saturating_sub(kept)))?;
    let mut tail = Vec::new();
    file.take(kept).read_to_end(&mut tail)?;
    let tail = String::from_utf8_lossy(&tail).into_owned();
    // An invalid byte reads as a longer replacement character.
    let size = size.max(tail.len() as u64);
    Ok((tail, size))
}

async fn receipt(context: &Context, project: &str, run: &str) -> Answer {
    let read = context.database.read().await?;
    let found = cockpit_verification::receipt(&read, project, run).await?;
    Ok(Reply::CheckReceipt {
        receipt: found.map(|receipt| Box::new(receipt_view(&receipt))),
    })
}

/// The gate of every check the project declares on the checkout's current result.
async fn gate(context: &Context, target: Target) -> Answer {
    let checks: Vec<CheckDefinition> = declared_checks(context, &target.path)
        .await?
        .iter()
        .map(definition)
        .collect();
    if checks.is_empty() {
        return Err(refused(
            ErrorCode::Refused,
            format!("project {} declares no checks", target.project),
            "declare them under `[[checks]]` in the project's .cockpit/config.toml",
            UnblockedBy::Owner,
        ));
    }
    let source = source(context, &target.path).await?;
    let evidence = {
        let read = context.database.read().await?;
        cockpit_verification::evidence(&read, &target.project, &source, &checks).await?
    };
    let checkout = target.path.clone();
    let report = tokio::task::spawn_blocking(move || evidence.evaluate(&checkout))
        .await
        .map_err(|error| internal(&error))??;
    Ok(Reply::CheckGate {
        gate: Box::new(gate_view(&target, &source, &report)),
    })
}

/// Records the sender's statement that `check` passed on its checkout's current result, as a
/// claim. The session needs `change-state` over itself, as for its other records.
async fn claim(
    context: &Context,
    connector: Result<ProcessIdentity, ProcessError>,
    launch_token: Option<String>,
    check: String,
    statement: String,
) -> Answer {
    let agent = Agent::identified(context, connector, launch_token).await?;
    let checks = declared_checks(context, &agent.target.path).await?;
    declared(&checks, &check, &agent.target.project)?;
    let source = source(context, &agent.target.path).await?;
    let write = context.database.write().await?;
    authorize(&write, &agent.session).await?;
    let claimed = cockpit_verification::record_claim(
        &write,
        NewClaim {
            project: agent.target.project.clone(),
            check_name: check,
            source,
            claimant: agent.session.as_str().to_owned(),
            statement,
        },
        now(),
    )
    .await?;
    write.commit().await?;
    Ok(Reply::CheckClaimed {
        claim: Box::new(claim_view(&agent.target.project, &claimed)),
    })
}

async fn authorize(connection: &Connection, session: &SessionId) -> Result<(), Failed> {
    let decision = cockpit_authorization::evaluate(
        connection,
        &cockpit_authorization::Actor::Session(session.clone()),
        Permission::ChangeState,
        &Scope::Session(session.clone()),
    )
    .await
    .map_err(|error| internal(&error))?;
    match decision.outcome {
        cockpit_authorization::Outcome::Allowed(_) => Ok(()),
        cockpit_authorization::Outcome::Refused(_) => Err(refused(
            ErrorCode::NotAuthorized,
            format!("session {} may not file a claim", session.as_str()),
            "ask the owner to grant change-state over the session",
            UnblockedBy::Owner,
        )),
    }
}

fn receipt_view(receipt: &Receipt) -> CheckReceiptView {
    let recorded = &receipt.recorded;
    CheckReceiptView {
        id: receipt.id.0,
        project: recorded.project.clone(),
        run: recorded.run_id.clone(),
        check: recorded.check.name.clone(),
        command: recorded.check.command.clone(),
        working_directory: recorded.check.working_directory.clone(),
        environment: recorded.check.environment.clone(),
        head: recorded.source.head.clone(),
        dirty: recorded.source.dirty.clone(),
        artifacts: recorded
            .artifacts
            .iter()
            .map(|artifact| CheckArtifactView {
                path: artifact.path.clone(),
                digest: artifact.digest.clone(),
            })
            .collect(),
        outcome: outcome_view(recorded.outcome),
        exit_code: recorded.exit_code,
        output: recorded.output.clone(),
        output_bytes: receipt.output_bytes,
        output_references: recorded.output_references.clone(),
        started_at: recorded.started_at,
        finished_at: recorded.finished_at,
    }
}

fn claim_view(project: &str, claim: &Claim) -> CheckClaimView {
    CheckClaimView {
        id: claim.id.0,
        project: project.to_owned(),
        check: claim.check_name.clone(),
        head: claim.source.head.clone(),
        dirty: claim.source.dirty.clone(),
        claimant: claim.claimant.clone(),
        statement: claim.statement.clone(),
        claimed_at: claim.claimed_at,
    }
}

fn gate_view(target: &Target, source: &Source, report: &GateReport) -> GateView {
    GateView {
        project: target.project.clone(),
        checkout: target.path.display().to_string(),
        head: source.head.clone(),
        dirty: source.dirty.clone(),
        outcome: match report.outcome {
            GateOutcome::Passed => GateOutcomeView::Passed,
            GateOutcome::Failed => GateOutcomeView::Failed,
            GateOutcome::Open => GateOutcomeView::Open,
        },
        checks: report
            .checks
            .iter()
            .map(|check| GateCheckView {
                name: check.name.clone(),
                status: status_view(&check.status),
                receipt: check.receipt.map(|receipt| receipt.0),
                next_action: check.next_action(),
            })
            .collect(),
    }
}

fn status_view(status: &CheckStatus) -> CheckStatusView {
    match status {
        CheckStatus::Checked(outcome) => CheckStatusView::Checked {
            outcome: outcome_view(*outcome),
        },
        CheckStatus::Stale(reason) => CheckStatusView::Stale {
            reason: match reason {
                StaleReason::CheckChanged => StaleReasonView::CheckChanged,
                StaleReason::SourceChanged { recorded } => StaleReasonView::SourceChanged {
                    head: recorded.head.clone(),
                    dirty: recorded.dirty.clone(),
                },
                StaleReason::ArtifactChanged { path } => {
                    StaleReasonView::ArtifactChanged { path: path.clone() }
                }
                StaleReason::ArtifactMissing { path } => {
                    StaleReasonView::ArtifactMissing { path: path.clone() }
                }
                StaleReason::ArtifactUnreadable { path, reason } => {
                    StaleReasonView::ArtifactUnreadable {
                        path: path.clone(),
                        detail: reason.clone(),
                    }
                }
            },
        },
        CheckStatus::Unverified => CheckStatusView::Unverified,
        CheckStatus::Missing => CheckStatusView::Missing,
    }
}

fn outcome_view(outcome: Outcome) -> CheckOutcomeView {
    match outcome {
        Outcome::Passed => CheckOutcomeView::Passed,
        Outcome::Failed => CheckOutcomeView::Failed,
        Outcome::Unavailable => CheckOutcomeView::Unavailable,
        Outcome::Blocked => CheckOutcomeView::Blocked,
        Outcome::Cancelled => CheckOutcomeView::Cancelled,
        Outcome::Unknown => CheckOutcomeView::Unknown,
    }
}

fn internal(error: &dyn std::fmt::Display) -> Failed {
    tracing::error!(%error, "a verification request failed");
    Failed(ProtocolError::new(
        ErrorCode::Internal,
        "the daemon failed; the request may or may not have taken effect",
    ))
}

impl From<Refusal> for Failed {
    fn from(refusal: Refusal) -> Failed {
        let code = match &refusal {
            Refusal::InvalidRequest { .. } => ErrorCode::InvalidArgument,
            Refusal::RunRecordedDifferently { .. } => ErrorCode::Conflict,
        };
        refused(
            code,
            refusal.to_string(),
            "fix the check's declaration or the request",
            UnblockedBy::Owner,
        )
    }
}

impl From<cockpit_verification::Error> for Failed {
    fn from(error: cockpit_verification::Error) -> Failed {
        match error {
            cockpit_verification::Error::Refused(refusal) => refusal.into(),
            error => internal(&error),
        }
    }
}
