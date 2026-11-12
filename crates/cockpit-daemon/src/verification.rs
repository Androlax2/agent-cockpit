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
use cockpit_processes::{ExecutionId, LaunchSpec, OutputKind, Supervisor};
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

mod environment;

/// A check's command is POSIX shell whatever the owner's login shell is, so a project's
/// checks read the same on every host.
const CHECK_SHELL: &str = "/bin/sh";

/// What a verification command needs: the state, the host Git that reads a checkout's source,
/// the supervisor that runs checks, how approved commands run (the shell and environment a
/// host check gets too), the sessions' owner settings and sandbox a check follows (#862), and
/// the owner's environment for the project's configuration.
#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) database: Arc<Database>,
    pub(crate) git: cockpit_projects::Git,
    pub(crate) processes: Supervisor,
    pub(crate) run_settings: Arc<RunSettings>,
    pub(crate) sessions: Arc<cockpit_sessions::Sessions>,
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
    let class = if matches!(command, Command::CheckPending { .. }) {
        WorkClass::Control
    } else {
        WorkClass::Bulk
    };
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
            (Command::CheckPending { project, after }, Peer::Owner) => {
                pending_checks(&context, &project, after.unwrap_or(0)).await
            }
            (
                Command::CheckResolvePending {
                    project,
                    run,
                    revision,
                    database_epoch,
                },
                Peer::Owner,
            ) => resolve_pending(&context, &project, &run, revision, &database_epoch).await,
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
                let refusal = run_refusal(&context, &checkout).await?;
                gate(&context, target(checkout), refusal).await
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
                // The owner runs the checks an agent's gate names, under the same refusal.
                let checkout =
                    owner_checkout(&context, &agent.target.project, agent.target.path.to_str())
                        .await?;
                let refusal = run_refusal(&context, &checkout).await?;
                gate(&context, target(checkout), refusal).await
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
        class,
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
                "register it with `torlek project add`",
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
                "name a path `torlek checkout list` shows",
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

/// What lets the owner run an untrusted checkout's checks.
const TRUST_STEP: &str = "trust the project under `projects.trusted` in the owner configuration";

/// Why `check run` would refuse every check of a checkout, so a gate's next actions say what
/// comes before running one (#649).
#[derive(Clone, Copy)]
enum RunRefusal {
    /// The owner trusting the project lifts it.
    Untrusted,
    /// Nothing lifts it: a review checkout holds someone else's changes.
    Review,
}

impl RunRefusal {
    /// `action`, a check's next action, preceded by what lifts the refusal.
    fn before(self, action: &str) -> String {
        match self {
            RunRefusal::Untrusted => format!("{TRUST_STEP}, then {action}"),
            RunRefusal::Review => format!(
                "in the project's own checkout or a parallel clone, {action}: a review \
                 checkout's checks never run"
            ),
        }
    }
}

/// A check runs the command the checkout's own configuration declares, on the host: only in
/// a checkout the owner trusts, never in a review checkout (report 09, report 11).
async fn require_trusted(
    context: &Context,
    checkout: &cockpit_projects::Checkout,
) -> Result<(), Failed> {
    if run_refusal(context, checkout).await?.is_none() {
        return Ok(());
    }
    Err(refused(
        ErrorCode::NotAuthorized,
        format!(
            "{} is not trusted: its checks are commands its own files declare",
            checkout.path.display()
        ),
        TRUST_STEP,
        UnblockedBy::Owner,
    ))
}

async fn run_refusal(
    context: &Context,
    checkout: &cockpit_projects::Checkout,
) -> Result<Option<RunRefusal>, Failed> {
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
    Ok(match cockpit_projects::trust(checkout, &trusted) {
        cockpit_projects::Trust::Trusted | cockpit_projects::Trust::Inherited { .. } => None,
        cockpit_projects::Trust::Untrusted => Some(RunRefusal::Untrusted),
        cockpit_projects::Trust::NeverForReview => Some(RunRefusal::Review),
    })
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
        let sender = Sender::read(connector).await;
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
                "work in a checkout `torlek project add` registered",
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
pub(crate) async fn declared_checks(
    environment: &cockpit_config::Environment,
    checkout: &Path,
) -> Result<Vec<cockpit_config::Check>, Failed> {
    let environment = environment.clone();
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

pub(crate) fn definition(check: &cockpit_config::Check) -> CheckDefinition {
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
                "declare it under `[[checks]]` in the project's .torlek/config.toml",
                UnblockedBy::Owner,
            )
        })
}

/// The checkout's head and uncommitted changes, read by the daemon's own Git.
pub(crate) async fn source(git: &cockpit_projects::Git, checkout: &Path) -> Result<Source, Failed> {
    let git = git.clone();
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

/// Starts `check` in `target` where the owner's `agents.sandbox` places it (#352) and answers
/// with its run; the receipt is recorded when it ends. A check placed nowhere is recorded
/// `unavailable` at once, with no process started.
async fn run(context: &Context, target: Target, check: &str) -> Answer {
    let checks = declared_checks(&context.environment, &target.path).await?;
    let check = declared(&checks, check, &target.project)?.clone();
    let definition = definition(&check);
    definition.validate()?;
    let start_dir = target.path.join(&definition.working_directory);
    let placement = environment::placement(
        context.sessions.owner_settings().sandbox,
        &definition.environment,
    );
    let sandboxed = match placement {
        environment::Placement::Sandbox => {
            Some(sandbox_inputs(context, &target, &start_dir).await?)
        }
        environment::Placement::Host | environment::Placement::Unavailable(_) => None,
    };
    let source = source(&context.git, &target.path).await?;
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
    cockpit_projects::require_execution_start_allowed(&write)
        .await
        .map_err(|error| {
            Failed(ProtocolError::new(
                crate::projects::error_code(&error),
                error.to_string(),
            ))
        })?;
    cockpit_projects::require_working_directory_available(&write, &target.path)
        .await
        .map_err(|error| {
            Failed(ProtocolError::new(
                crate::projects::error_code(&error),
                error.to_string(),
            ))
        })?;
    let execution = context
        .processes
        .reserve(&write, context.database.epoch())
        .await
        .map_err(|error| internal(&error))?;
    let started_at = now();
    let run_id = format!("execution-{}-{}", context.database.epoch(), execution.0);
    // Prepared under the writer, since its folders are named for the run; it writes nothing.
    let launch = match (placement, sandboxed) {
        (environment::Placement::Unavailable(reason), _) => Err(reason.to_owned()),
        (environment::Placement::Sandbox, Some(inputs)) => {
            sandboxed_launch(context, &run_id, &definition, &target, start_dir, inputs)
        }
        (environment::Placement::Sandbox, None) => {
            unreachable!("a sandboxed placement read its inputs")
        }
        (environment::Placement::Host, _) => {
            Ok(host_launch(context, &definition, start_dir, &git_config))
        }
    };
    cockpit_processes::attribute_output(
        &write,
        execution,
        context.database.epoch(),
        Some(&target.project),
        None,
        cockpit_processes::OutputMode::Files,
        true,
    )
    .await
    .map_err(|e| internal(&e))?;
    let pending = cockpit_verification::reserve_result(
        &write,
        context.database.epoch(),
        execution.0,
        NewReceipt {
            project: target.project,
            run_id: run_id.clone(),
            check: definition.clone(),
            ran: launch.as_ref().ok().map(|launch| launch.ran.clone()),
            source,
            artifacts,
            outcome: Outcome::Unknown,
            exit_code: None,
            output: String::new(),
            output_references: Vec::new(),
            started_at,
            finished_at: started_at,
        },
    )
    .await?;
    let launch = match launch {
        Ok(launch) => launch,
        Err(reason) => {
            tracing::info!(run = run_id, check = definition.name, %reason, "check unavailable");
            seal_never_started(&write, &pending, &reason).await?;
            write.commit().await?;
            return Ok(Reply::CheckRunStarted { run: run_id });
        }
    };
    let collector =
        cockpit_processes::guard_output_in(&write, Arc::clone(&context.database), execution)
            .await
            .map_err(|e| internal(&e))?;
    write.commit().await?;
    if let Some(staged) = launch.staged.clone() {
        let folder = launch.folder.as_ref().map(|folder| folder.root.clone());
        let written = tokio::task::spawn_blocking(move || {
            if let Some(folder) = &folder {
                create_owner_folder(&folder.join("tmp"))?;
            }
            staged.write()
        })
        .await
        .map_err(|error| internal(&error))?;
        if let Err(error) = written {
            // Nothing started, and this run's own collector is the only one: dropped first.
            drop(collector);
            let write = context.database.write().await?;
            seal_never_started(
                &write,
                &pending,
                &format!("the sandbox could not start: {error}"),
            )
            .await?;
            write.commit().await?;
            remove_run_folders(launch.cleanup).await;
            return Ok(Reply::CheckRunStarted { run: run_id });
        }
    }
    if let cockpit_sandbox::Enforcement::Unsandboxed { reason } = &launch.enforcement {
        tracing::warn!(
            run = run_id,
            %reason,
            "a check runs UNSANDBOXED: {}",
            cockpit_config::UNSANDBOXED_RISK
        );
    }
    match context
        .processes
        .launch_reserved_with_output(execution, &launch.spec, collector.clone())
        .await
    {
        Ok(launched) => {
            tokio::spawn(record_when_ended(
                context.clone(),
                pending,
                launched.exit,
                collector,
                launch.cleanup,
            ));
        }
        Err(error) => {
            let write = context.database.write().await?;
            let observed = cockpit_processes::execution_in(&write, execution)
                .await
                .map_err(|e| internal(&e))?;
            if observed.phase
                == cockpit_processes::Phase::Ended(cockpit_processes::EndReason::NeverStarted)
            {
                let mut result = pending.receipt.clone();
                result.outcome = Outcome::Unavailable;
                result.ran = None;
                result.finished_at = now().max(result.started_at);
                result.output = format!("check could not start: {error}");
                let bytes = result.output.len() as u64;
                cockpit_verification::seal_result(
                    &write,
                    &pending,
                    result,
                    bytes,
                    now().max(pending.receipt.started_at),
                    false,
                )
                .await?;
                cockpit_processes::seal_output_result(&write, execution, &pending.origin_epoch)
                    .await
                    .map_err(|e| internal(&e))?;
                write.commit().await?;
                remove_run_folders(launch.cleanup).await;
                return Err(refused(
                    ErrorCode::Refused,
                    format!("check `{}` could not start: {error}", definition.name),
                    "make the check's working directory and launcher available",
                    UnblockedBy::Owner,
                ));
            }
            drop(write);
            // No guessed outcome: even a failed launch has a durable, recoverable intent.
            // Pending protection remains until an observed-inactive owner recovery seals it.
            // Its folders stay too: the launch may still run in them.
            tracing::warn!(run = run_id, %error, "check launch uncertain; pending result retained");
        }
    }
    Ok(Reply::CheckRunStarted { run: run_id })
}

/// What a check run needs from the host before it starts: its launch, where it runs, and
/// what to remove when it ends.
struct CheckLaunch {
    spec: LaunchSpec,
    ran: cockpit_verification::Ran,
    enforcement: cockpit_sandbox::Enforcement,
    staged: Option<cockpit_sandbox::Staged>,
    folder: Option<environment::RunFolder>,
    cleanup: Vec<PathBuf>,
}

/// On the host, with the owner's approved commands' shell and environment, as before #352.
fn host_launch(
    context: &Context,
    definition: &CheckDefinition,
    start_dir: PathBuf,
    git_config: &[String],
) -> CheckLaunch {
    CheckLaunch {
        spec: LaunchSpec {
            program: CHECK_SHELL.into(),
            arguments: vec![
                "-c".into(),
                format!("exec 2>&1\n{}", definition.command).into(),
            ],
            working_dir: start_dir,
            environment: context.run_settings.run_environment(git_config),
            terminal: None,
        },
        ran: environment::ran_on_host(),
        enforcement: cockpit_sandbox::Enforcement::Unsandboxed {
            reason: "no sandbox: agents.sandbox = \"off\"".to_owned(),
        },
        staged: None,
        folder: None,
        cleanup: Vec::new(),
    }
}

/// What a sandboxed run reads before the writer: the checkout's toolchains, the registry
/// hosts they fetch from and the project's host grants (#862, Q2).
struct SandboxInputs {
    tools: cockpit_sandbox::ToolNeeds,
    egress: Vec<cockpit_sandbox::AllowEntry>,
    decisions: cockpit_sandbox::EgressDecisions,
}

async fn sandbox_inputs(
    context: &Context,
    target: &Target,
    start_dir: &Path,
) -> Result<SandboxInputs, Failed> {
    let checkout = target.path.clone();
    let start = start_dir.to_path_buf();
    let daemon = context.run_settings.environment.clone();
    let tools = tokio::task::spawn_blocking(move || {
        cockpit_sessions::tool_needs(&checkout, &start, &daemon)
    })
    .await
    .map_err(|error| internal(&error))?
    .map_err(|error| {
        refused(
            ErrorCode::Refused,
            format!("the checkout's toolchains cannot be read: {error}"),
            "fix the toolchain files in the checkout",
            UnblockedBy::Owner,
        )
    })?;
    let egress = cockpit_sessions::registry_entries(&tools.ecosystems).map_err(|reason| {
        refused(
            ErrorCode::Refused,
            reason,
            "report it: the toolchain table is Torlek's own",
            UnblockedBy::Owner,
        )
    })?;
    let grants = {
        let read = context.database.read().await?;
        cockpit_sandbox::network_grants(&read, Some(&target.project))
            .await
            .map_err(|error| internal(&error))?
    };
    Ok(SandboxInputs {
        tools,
        egress,
        decisions: environment::host_grants(grants),
    })
}

/// Through `torlek-sandbox` (#862): the reason it cannot when the sandbox refuses it, which the
/// receipt records as `unavailable`.
fn sandboxed_launch(
    context: &Context,
    run_id: &str,
    definition: &CheckDefinition,
    target: &Target,
    start_dir: PathBuf,
    inputs: SandboxInputs,
) -> Result<CheckLaunch, String> {
    let sandbox = context.sessions.sandbox();
    let folder = environment::RunFolder::new(context.sessions.session_temps(), run_id);
    let request = cockpit_sandbox::CheckRequest {
        run: run_id.to_owned(),
        check: definition.name.clone(),
        shell: CHECK_SHELL.into(),
        command: definition.command.clone(),
        checkout: target.path.clone(),
        start_dir: start_dir.clone(),
        execution_dir: folder.root.clone(),
        temp_dir: folder.temp_dir(),
        environment: environment::sandboxed_environment(
            &context.run_settings.environment,
            &folder.temp_dir(),
        ),
        tools: inputs.tools,
        egress: inputs.egress,
        decisions: inputs.decisions,
    };
    let prepared = sandbox
        .prepare_check(cockpit_config::AgentSandbox::Required, &request)
        .map_err(|refusal| format!("the sandbox cannot start: {refusal}"))?;
    let cockpit_sandbox::Backend::Native(backend) = sandbox.backend() else {
        return Err("the sandbox prepared a launch without a backend".to_owned());
    };
    let staged = prepared
        .staged
        .clone()
        .ok_or_else(|| "the sandbox prepared no launch spec".to_owned())?;
    let enforcement = prepared.evidence.enforcement.clone();
    Ok(CheckLaunch {
        ran: environment::ran_in_sandbox(backend),
        enforcement,
        cleanup: vec![
            folder.root.clone(),
            staged.mount_dir.clone(),
            staged.home_source.clone(),
        ],
        spec: environment::launcher_spec(prepared, start_dir),
        staged: Some(staged),
        folder: Some(folder),
    })
}

/// Seals `pending` as `unavailable` with `reason` as its output and ends its reserved
/// execution as never started, in the caller's transaction: no process of it ever ran. The
/// caller holds no collector of its output.
async fn seal_never_started(
    write: &cockpit_turso::WriteTransaction<'_>,
    pending: &cockpit_verification::PendingResult,
    reason: &str,
) -> Result<(), Failed> {
    let execution = ExecutionId(pending.execution);
    let ended = cockpit_processes::end_unstarted_output(write, execution, &pending.origin_epoch)
        .await
        .map_err(|e| internal(&e))?;
    if !ended {
        return Err(internal(&"a check run that never launched was not ended"));
    }
    let mut result = pending.receipt.clone();
    result.outcome = Outcome::Unavailable;
    result.ran = None;
    result.finished_at = now().max(result.started_at);
    result.output = reason.to_owned();
    let bytes = result.output.len() as u64;
    cockpit_verification::seal_result(
        write,
        pending,
        result,
        bytes,
        now().max(pending.receipt.started_at),
        false,
    )
    .await?;
    cockpit_processes::seal_output_result(write, execution, &pending.origin_epoch)
        .await
        .map_err(|e| internal(&e))?;
    Ok(())
}

fn create_owner_folder(folder: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
}

/// Removes a sandboxed run's own folders once it ended (#859); a failure is logged, since the
/// receipt is already recorded.
async fn remove_run_folders(folders: Vec<PathBuf>) {
    if folders.is_empty() {
        return;
    }
    let removed = tokio::task::spawn_blocking(move || {
        for folder in folders {
            match cockpit_sessions::remove_tree(&folder) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::warn!(folder = %folder.display(), %error, "a check run's folder was not removed");
                }
            }
        }
    })
    .await;
    if let Err(error) = removed {
        tracing::warn!(%error, "a check run's folders were not removed");
    }
}

async fn record_when_ended(
    context: Context,
    pending: cockpit_verification::PendingResult,
    exit: oneshot::Receiver<std::io::Result<ExitStatus>>,
    collector: cockpit_processes::OutputGuard,
    cleanup: Vec<PathBuf>,
) {
    let mut receipt = pending.receipt.clone();
    (receipt.outcome, receipt.exit_code) = match exit.await {
        Ok(Ok(status)) => match status.code() {
            Some(0) => (Outcome::Passed, Some(0)),
            Some(code) => (Outcome::Failed, Some(i64::from(code))),
            None => (Outcome::Failed, None),
        },
        // The pending hold remains, and the run's folders with it: no timeout success.
        Ok(Err(_)) | Err(_) => return,
    };
    receipt.finished_at = now();
    let execution = ExecutionId(pending.execution);
    let path = context
        .processes
        .output_path(&context.database, execution, OutputKind::Stdout);
    receipt.output_references = vec![path.display().to_string()];
    // A cancelled collector cannot release the opener/reader until the actual blocking read ends.
    let read_guard = collector.clone();
    let read = tokio::task::spawn_blocking(move || {
        let _guard = read_guard;
        output_tail(&path)
    })
    .await;
    let bytes = match read {
        Ok(Ok((tail, bytes))) => {
            receipt.output = tail;
            bytes
        }
        Ok(Err(error)) => {
            receipt.output = format!("torlek could not read the output: {error}");
            receipt.output.len() as u64
        }
        Err(error) => {
            receipt.output = format!("torlek could not read the output: {error}");
            receipt.output.len() as u64
        }
    };
    let recorded = async {
        let write = context.database.write().await?;
        let receipt =
            cockpit_verification::seal_result(&write, &pending, receipt, bytes, now(), false)
                .await?;
        cockpit_processes::seal_output_result(&write, execution, &pending.origin_epoch).await?;
        write.commit().await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(receipt)
    }
    .await;
    match recorded {
        Ok(receipt) => tracing::info!(
            run = receipt.recorded.run_id,
            receipt = receipt.id.0,
            "recorded check result"
        ),
        Err(error) => {
            tracing::error!(run = pending.receipt.run_id, %error, "pending check result could not be sealed")
        }
    }
    drop(collector);
    remove_run_folders(cleanup).await;
    // Output retention only considers ended runs; without this the run stays running until
    // the next reservation sweep.
    if let Err(error) = context
        .processes
        .observe(&context.database, execution)
        .await
    {
        tracing::warn!(run = pending.receipt.run_id, %error, "check run not observed ended");
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
    let mut found = cockpit_verification::receipt(&read, project, run).await?;
    if found.is_none()
        && let Some(pending) = cockpit_verification::pending_result(&read, project, run).await?
        && cockpit_processes::output_users(context.database.epoch(), ExecutionId(pending.execution))
            == 0
    {
        // The collector may have committed and dropped its guard after this read
        // snapshot began. Serialize a fresh orphan check with sealing/guard acquisition;
        // ordinary receipt and active-collector polls remain read-only.
        drop(read);
        let write = context.database.write().await?;
        found = cockpit_verification::receipt(&write, project, run).await?;
        if found.is_none()
            && let Some(current) =
                cockpit_verification::pending_result(&write, project, run).await?
            && cockpit_processes::output_users(
                context.database.epoch(),
                ExecutionId(current.execution),
            ) == 0
        {
            return Err(refused(
                ErrorCode::Conflict,
                "the result remains pending without an active collector".into(),
                "inspect torlek check pending --project PROJECT; explicitly resolve only after containment is observed inactive",
                UnblockedBy::Owner,
            ));
        }
    }
    let output_retired = match &found {
        Some(_) => output_retired(context, project, run).await?,
        None => false,
    };
    Ok(Reply::CheckReceipt {
        receipt: found.map(|receipt| Box::new(receipt_view(&receipt, output_retired))),
    })
}

/// Whether output retention removed this run's full output (#406). It only retires its own
/// database epoch's outputs, so a run of an earlier epoch keeps its files.
async fn output_retired(context: &Context, project: &str, run: &str) -> Result<bool, Failed> {
    let read = context.database.read().await?;
    let Some(identity) = cockpit_verification::pending_result(&read, project, run).await? else {
        return Ok(false);
    };
    if identity.origin_epoch != context.database.epoch() {
        return Ok(false);
    }
    let facts = cockpit_processes::output_facts(&read, ExecutionId(identity.execution))
        .await
        .map_err(|e| internal(&e))?;
    Ok(facts.retirement.is_some())
}

/// The gate of every check the project declares on the checkout's current result; each check's
/// next action says first what lifts `refusal`, when `check run` would refuse it.
async fn gate(context: &Context, target: Target, refusal: Option<RunRefusal>) -> Answer {
    let checks: Vec<CheckDefinition> = declared_checks(&context.environment, &target.path)
        .await?
        .iter()
        .map(definition)
        .collect();
    if checks.is_empty() {
        return Err(refused(
            ErrorCode::Refused,
            format!("project {} declares no checks", target.project),
            "declare them under `[[checks]]` in the project's .torlek/config.toml",
            UnblockedBy::Owner,
        ));
    }
    let source = source(&context.git, &target.path).await?;
    let evidence = {
        let read = context.database.read().await?;
        cockpit_verification::evidence(&read, &target.project, &source, &checks).await?
    };
    let checkout = target.path.clone();
    let report = tokio::task::spawn_blocking(move || evidence.evaluate(&checkout))
        .await
        .map_err(|error| internal(&error))??;
    // A check placed nowhere is not lifted by running it again: its next action says what is.
    let setting = context.sessions.owner_settings().sandbox;
    let unplaced: Vec<&str> = checks
        .iter()
        .filter(|check| {
            environment::placement(setting, &check.environment)
                == environment::Placement::Unavailable(environment::NATIVE_UNDER_REQUIRED)
        })
        .map(|check| check.name.as_str())
        .collect();
    Ok(Reply::CheckGate {
        gate: Box::new(gate_view(&target, &source, &report, refusal, &unplaced)),
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
    let checks = declared_checks(&context.environment, &agent.target.path).await?;
    declared(&checks, &check, &agent.target.project)?;
    let source = source(&context.git, &agent.target.path).await?;
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

fn receipt_view(receipt: &Receipt, output_retired: bool) -> CheckReceiptView {
    let recorded = &receipt.recorded;
    CheckReceiptView {
        id: receipt.id.0,
        project: recorded.project.clone(),
        run: recorded.run_id.clone(),
        check: recorded.check.name.clone(),
        command: recorded.check.command.clone(),
        working_directory: recorded.check.working_directory.clone(),
        environment: recorded.check.environment.clone(),
        ran_environment: recorded.ran.as_ref().map(|ran| ran.environment.clone()),
        ran_isolation: recorded.ran.as_ref().map(|ran| ran.isolation.clone()),
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
        output_retired,
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

/// `native_under_required` names the checks that declare `native` under `agents.sandbox =
/// "required"`, whose next action is to declare another environment or turn the sandbox off.
fn gate_view(
    target: &Target,
    source: &Source,
    report: &GateReport,
    refusal: Option<RunRefusal>,
    native_under_required: &[&str],
) -> GateView {
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
                next_action: check
                    .next_action()
                    .map(|action| {
                        if native_under_required.contains(&check.name.as_str()) {
                            environment::NATIVE_NEXT_ACTION.to_owned()
                        } else {
                            action
                        }
                    })
                    .map(|action| match refusal {
                        Some(refusal) => refusal.before(&action),
                        None => action,
                    }),
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
            Refusal::PendingResultChanged => ErrorCode::StaleRevision,
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

async fn pending_checks(context: &Context, project: &str, after: i64) -> Answer {
    if after < 0 {
        return Err(refused(
            ErrorCode::InvalidArgument,
            "negative pending cursor".into(),
            "use the displayed next cursor",
            UnblockedBy::Owner,
        ));
    }
    let read = context.database.read().await?;
    let mut rows = cockpit_verification::pending_results(&read, project, after, 101).await?;
    let truncated = rows.len() > 100;
    rows.truncate(100);
    let next = if truncated {
        rows.last().map(|row| row.execution)
    } else {
        None
    };
    let mut pending = Vec::new();
    for row in rows {
        let execution = ExecutionId(row.execution);
        let facts = cockpit_processes::output_facts(&read, execution)
            .await
            .map_err(|e| internal(&e))?;
        pending.push(cockpit_protocol::PendingCheckView {
            project: project.to_owned(),
            run: row.run_id,
            execution: row.execution,
            origin_epoch: row.origin_epoch,
            database_epoch: context.database.epoch().to_owned(),
            revision: row.revision,
            active: facts.active,
            collector_active: cockpit_processes::output_users(context.database.epoch(), execution)
                != 0,
        });
    }
    Ok(Reply::CheckPending { pending, next })
}

async fn resolve_pending(
    context: &Context,
    project: &str,
    run: &str,
    revision: i64,
    epoch: &str,
) -> Answer {
    let stale = || {
        refused(
            ErrorCode::StaleRevision,
            "pending result identity changed".into(),
            "inspect check pending and deliberately choose its current epoch/revision",
            UnblockedBy::Owner,
        )
    };
    if epoch != context.database.epoch() || revision <= 0 {
        return Err(stale());
    }
    let pending = {
        let read = context.database.read().await?;
        cockpit_verification::pending_result(&read, project, run)
            .await?
            .ok_or_else(stale)?
    };
    let execution = ExecutionId(pending.execution);
    if !pending.sealed {
        if pending.revision != revision {
            return Err(stale());
        }
        if cockpit_processes::output_users(context.database.epoch(), execution) != 0 {
            return Err(refused(
                ErrorCode::Conflict,
                "execution or result reader is still active".into(),
                "wait for the actual collector to finish",
                UnblockedBy::Owner,
            ));
        }
        context
            .processes
            .observe(&context.database, execution)
            .await
            .map_err(|e| internal(&e))?;
    }
    let write = context.database.write().await?;
    let current = cockpit_verification::pending_result(&write, project, run)
        .await?
        .ok_or_else(stale)?;
    if current.sealed {
        // Recovery replays cannot rewrite normal successful history, and old callbacks remain fenced.
        if current.revision != revision
            && !(current.recovered && Some(current.revision) == revision.checked_add(1))
        {
            return Err(stale());
        }
        let receipt = cockpit_verification::receipt(&write, project, run)
            .await?
            .ok_or_else(stale)?;
        return Ok(Reply::CheckReceipt {
            receipt: Some(Box::new(receipt_view(&receipt, false))),
        });
    }
    if current != pending || current.revision != revision {
        return Err(stale());
    }
    let has_collector = cockpit_processes::output_users(context.database.epoch(), execution) != 0;
    let ended = if has_collector {
        false
    } else {
        cockpit_processes::end_unstarted_output(&write, execution, &current.origin_epoch)
            .await
            .map_err(|e| internal(&e))?
            || cockpit_processes::observed_ended(&write, &[execution])
                .await
                .map_err(|e| internal(&e))?
    };
    if !ended {
        return Err(refused(
            ErrorCode::Conflict,
            "execution or result reader is still active or unobserved".into(),
            "wait for observed containment termination and the actual collector to finish",
            UnblockedBy::Owner,
        ));
    }
    let mut result = current.receipt.clone();
    result.finished_at = now().max(result.started_at);
    result.output =
        "Owner resolved an inactive pending result as unknown; output files were preserved."
            .to_owned();
    let bytes = result.output.len() as u64;
    let receipt = cockpit_verification::seal_result(
        &write,
        &current,
        result,
        bytes,
        now().max(current.receipt.started_at),
        true,
    )
    .await?;
    cockpit_processes::seal_output_result(&write, execution, &current.origin_epoch)
        .await
        .map_err(|e| internal(&e))?;
    write.commit().await?;
    Ok(Reply::CheckReceipt {
        receipt: Some(Box::new(receipt_view(&receipt, false))),
    })
}
