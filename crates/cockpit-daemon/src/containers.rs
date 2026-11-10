//! A project's own container engine (#390): the daemon finds the project, its clones folder
//! and the owner, and `cockpit-containers` verifies the engine answering on the project
//! user's socket against the one the owner recorded. Serves `containers-record` and the
//! readiness report's `docker` line under `agents.sandbox = "required"`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cockpit_containers::{
    EngineState, EngineUser, Expected, PASSWD_FILE, RecordedEngine, Remedy, SOCKET_DEADLINE,
    SetupPlan,
};
use cockpit_projects::Project;
use cockpit_protocol::{
    Command, DecidedThroughView, ErrorCode, ProtocolError, Reply, Unblock, UnblockedBy,
};
use cockpit_readiness::{EngineRemedy, ProjectEngine};
use cockpit_turso::Database;
use cockpit_turso::sql::Connection;

use crate::approvals::{Failed, internal, job, now};
use crate::routes::{Route, WorkClass};

/// Who owns this daemon and where it writes, as an engine's checks and setup need them.
#[derive(Debug, Clone)]
pub(crate) struct Host {
    pub(crate) owner_uid: u32,
    /// The daemon's `HOME`, which holds the owner's checkouts: the setup script hides it from
    /// the engine and allows only checkouts inside it. Without one, the owner's account's.
    pub(crate) owner_home: Option<PathBuf>,
    /// `torlek` or `torlek-<profile>`: the prefix of every engine user and the command name.
    pub(crate) qualified_profile: String,
    /// Where setup scripts are written for the owner to review.
    pub(crate) scripts_dir: PathBuf,
    /// The installed `torlek-sandbox` the engine's relay unit runs; the setup script checks
    /// the engine's user can run it.
    pub(crate) relay_program: PathBuf,
}

impl Host {
    pub(crate) fn new(
        owner_uid: u32,
        state_dir: &Path,
        environment: &cockpit_config::Environment,
        sandbox_launcher: &Path,
    ) -> Host {
        Host {
            owner_uid,
            owner_home: environment.home.as_ref().map(PathBuf::from),
            qualified_profile: environment.profile.value.qualified_name(),
            scripts_dir: state_dir.join("containers"),
            relay_program: sandbox_launcher.to_path_buf(),
        }
    }
}

/// The route for `containers-record`; `routes.rs` sends only that here, from the host-only
/// socket. The engine is asked outside any transaction: it may take [`SOCKET_DEADLINE`].
pub(crate) fn route(command: Command, database: Arc<Database>, host: Host) -> Route {
    let Command::ContainersRecord { project, through } = command else {
        unreachable!("routes.rs sends only containers-record here, not {command:?}")
    };
    let work = job(async move {
        let found = {
            let read = database.read().await?;
            registered(&read, &project).await?
        };
        let passwd = read_passwd().await.map_err(|reason| internal(&reason))?;
        let user = engine_user(&host, &found)?;
        // The record is not consulted: recording again is how the owner accepts a new engine.
        let state = examine(&host, &user, &passwd, None).await;
        let EngineState::Unrecorded(info) = state else {
            return Err(not_recordable(&state, &user, &found, &host));
        };
        let engine = RecordedEngine {
            project: project.clone(),
            user: user.name().to_owned(),
            engine_id: info.id,
            recorded_at: now(),
            recorded_through: through.map(through_name),
        };
        let write = database.write().await?;
        let replaced = cockpit_containers::record_engine(&write, &engine).await?;
        write.commit().await?;
        tracing::info!(
            %project,
            user = %engine.user,
            engine = %engine.engine_id,
            replaced = ?replaced.as_ref().map(|old| &old.engine_id),
            "the owner recorded a project's engine"
        );
        Ok(Reply::ContainerEngineRecorded {
            project,
            user: engine.user,
            engine_id: engine.engine_id,
            replaced: replaced.map(|old| old.engine_id),
        })
    });
    Route::Run {
        class: WorkClass::Control,
        job: work,
    }
}

/// The readiness report's view of `project`'s engine: verified against the owner's record,
/// with the setup script written when setting up is the next step.
pub(crate) async fn project_engine(
    database: &Database,
    host: &Host,
    project: &str,
) -> Result<ProjectEngine, String> {
    let (found, recorded) = {
        let read = database.read().await.map_err(|error| error.to_string())?;
        let found = cockpit_projects::project_named(&read, project)
            .await
            .map_err(|error| error.to_string())?;
        let recorded = cockpit_containers::recorded_engine(&read, project)
            .await
            .map_err(|error| error.to_string())?;
        (found, recorded)
    };
    let passwd = read_passwd().await?;
    let user = EngineUser::for_project(&host.qualified_profile, found.id.0)?;
    let state = examine(host, &user, &passwd, recorded.as_ref()).await;
    let record_command = record_command(host, &found);
    let setup = || {
        let (host, found, user, passwd, record_command) = (
            host.clone(),
            found.clone(),
            user.clone(),
            passwd.clone(),
            record_command.clone(),
        );
        async move {
            tokio::task::spawn_blocking(move || {
                write_setup(&host, &found, &user, &passwd, &record_command)
            })
            .await
            .map_err(|error| error.to_string())?
        }
    };
    let state = match state {
        EngineState::NotSetUp { reason } => cockpit_readiness::EngineState::NotSetUp {
            reason,
            setup: setup().await,
        },
        EngineState::Unrecorded(info) => {
            cockpit_readiness::EngineState::Unrecorded(crate::readiness::docker_engine(info))
        }
        EngineState::Verified(info) => {
            cockpit_readiness::EngineState::Verified(crate::readiness::docker_engine(info))
        }
        EngineState::Refused(refusal) => cockpit_readiness::EngineState::Refused {
            reason: refusal.to_string(),
            remedy: match refusal.remedy() {
                Remedy::Setup => EngineRemedy::Setup(setup().await),
                Remedy::Start => EngineRemedy::Start { unit: user.unit() },
                Remedy::Record => EngineRemedy::Record,
            },
        },
    };
    Ok(ProjectEngine {
        project: found.name.as_str().to_owned(),
        user: user.name().to_owned(),
        record_command,
        state,
    })
}

/// What answers on `user`'s socket, checked against `recorded`.
async fn examine(
    host: &Host,
    user: &EngineUser,
    passwd: &str,
    recorded: Option<&RecordedEngine>,
) -> EngineState {
    let user_uid = cockpit_containers::account_named(passwd, user.name()).map(|found| found.uid);
    let answer = cockpit_containers::ask_socket(&user.socket(), SOCKET_DEADLINE).await;
    cockpit_containers::verify(
        answer,
        Expected {
            user,
            user_uid,
            owner_uid: host.owner_uid,
        },
        recorded,
    )
}

/// `torlek containers record NAME`, under this profile's command name.
fn record_command(host: &Host, project: &Project) -> String {
    format!(
        "{} containers record {}",
        host.qualified_profile, project.name
    )
}

/// Writes the setup script for `project`'s engine; its path, or why it was not written.
/// Blocks on the file system.
fn write_setup(
    host: &Host,
    project: &Project,
    user: &EngineUser,
    passwd: &str,
    record_command: &str,
) -> Result<PathBuf, String> {
    let mut owner = cockpit_containers::account_with_uid(passwd, host.owner_uid)
        .ok_or_else(|| format!("uid {} has no account in {PASSWD_FILE}", host.owner_uid))?;
    if let Some(home) = &host.owner_home {
        owner.home.clone_from(home);
    }
    let clones_dir = cockpit_projects::clones_dir(&project.root, &project.name)
        .map_err(|error| error.to_string())?;
    let script = cockpit_containers::setup_script(&SetupPlan {
        project: project.name.as_str(),
        user,
        owner: &owner,
        project_root: &project.root,
        clones_dir: &clones_dir,
        record_command,
        relay_program: &host.relay_program,
    })?;
    cockpit_containers::write_setup_script(&host.scripts_dir, user, &script)
}

async fn read_passwd() -> Result<String, String> {
    tokio::fs::read_to_string(PASSWD_FILE)
        .await
        .map_err(|error| format!("cannot read {PASSWD_FILE}: {error}"))
}

fn engine_user(host: &Host, project: &Project) -> Result<EngineUser, Failed> {
    EngineUser::for_project(&host.qualified_profile, project.id.0)
        .map_err(|reason| Failed(ProtocolError::new(ErrorCode::Refused, reason)))
}

async fn registered(connection: &Connection, project: &str) -> Result<Project, Failed> {
    match cockpit_projects::project_named(connection, project).await {
        Ok(found) => Ok(found),
        Err(cockpit_projects::Error::UnknownProject { .. }) => Err(Failed(ProtocolError {
            code: ErrorCode::NotFound,
            message: format!("no registered project {project}"),
            unblock: Some(Unblock {
                action: "name a project `torlek project list` shows".to_owned(),
                by: UnblockedBy::Owner,
            }),
        })),
        Err(error) => Err(internal(&error)),
    }
}

/// Why an engine that is not the project's own, rootless and answering is not recorded.
fn not_recordable(
    state: &EngineState,
    user: &EngineUser,
    project: &Project,
    host: &Host,
) -> Failed {
    let doctor = format!(
        "`{} doctor --project {}` writes the setup script to review and run",
        host.qualified_profile,
        project.root.display()
    );
    let (message, action) = match state {
        EngineState::NotSetUp { reason } => (format!("no engine to record: {reason}"), doctor),
        EngineState::Refused(refusal) => (
            format!("the engine of {user} is not recorded: {refusal}"),
            match refusal.remedy() {
                Remedy::Start => format!("`sudo systemctl start {}`, then record it", user.unit()),
                Remedy::Setup | Remedy::Record => doctor,
            },
        ),
        EngineState::Unrecorded(_) | EngineState::Verified(_) => {
            unreachable!("an engine examined without a record is recordable or refused")
        }
    };
    Failed(ProtocolError {
        code: ErrorCode::Refused,
        message,
        unblock: Some(Unblock {
            action,
            by: UnblockedBy::Owner,
        }),
    })
}

fn through_name(through: DecidedThroughView) -> String {
    match through {
        DecidedThroughView::Tui => "tui",
        DecidedThroughView::Cli => "cli",
    }
    .to_owned()
}
