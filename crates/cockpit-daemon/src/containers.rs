//! A project's own container engine (#390): the daemon finds the project, its clones folder
//! and the owner, and `cockpit-containers` verifies the engine answering on the project
//! user's socket against the one the owner recorded. Serves `containers-record` and the
//! readiness report's `docker` line under `agents.sandbox = "required"`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cockpit_config::PORT_BASE_VARIABLE;
use cockpit_containers::{
    EngineState, EngineUser, Expected, MASKS_DEADLINE, MasksHelper, MasksUnready, PASSWD_FILE,
    RecordedEngine, Remedy, SOCKET_DEADLINE, SetupPlan,
};
use cockpit_projects::Project;
use cockpit_protocol::{
    Command, DecidedThroughView, ErrorCode, ProtocolError, Reply, Unblock, UnblockedBy,
};
use cockpit_readiness::{EngineRemedy, MasksGap, ProjectEngine};
use cockpit_sandbox::{EngineRelay, EngineSocket, Forward};
use cockpit_sessions::HeldPortBlock;
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
    /// the engine's user can run it. `torlek setup engine-masks` copies it as the masks helper.
    pub(crate) relay_program: PathBuf,
    /// Torlek's masks helper, which shows an engine each session's checkout masked.
    pub(crate) masks: MasksHelper,
    /// The state of each recorded engine's egress proxy, which `engine_proxies.rs` runs.
    pub(crate) proxies: crate::engine_proxies::EngineProxies,
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
            masks: MasksHelper::installed(),
            proxies: crate::engine_proxies::EngineProxies::default(),
        }
    }

    /// The user `project`'s engine runs as in this profile: what asks, records and the
    /// launchers' decisions all name.
    fn engine_user(&self, project: &Project) -> Result<EngineUser, String> {
        EngineUser::for_project(&self.qualified_profile, project.id.0)
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

/// The root step `torlek setup engine-masks` runs with sudo (#390): it copies the
/// `torlek-sandbox` this daemon runs, which readiness compares the installed helper with.
pub(crate) fn masks_helper_step(host: &Host, uninstall: bool) -> Reply {
    Reply::MasksHelperStep {
        script: if uninstall {
            cockpit_containers::masks_removal_script()
        } else {
            cockpit_containers::masks_setup_script()
        },
        helper: host.relay_program.display().to_string(),
    }
}

/// The readiness report's view of `project`'s engine: verified against the owner's record,
/// with the setup script written when setting up is the next step.
pub(crate) async fn project_engine(
    database: &Database,
    host: &Host,
    project: &str,
) -> Result<ProjectEngine, String> {
    let (found, recorded) = project_and_record(database, project).await?;
    let passwd = read_passwd().await?;
    let user = host.engine_user(&found)?;
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
        EngineState::Verified(info) => cockpit_readiness::EngineState::Verified {
            engine: crate::readiness::docker_engine(info),
            masks: match host
                .masks
                .check_ready(host.owner_uid, &user, &host.relay_program)
            {
                Ok(()) => Ok(()),
                Err(MasksUnready::NotInstalled(reason)) => Err(MasksGap::NotInstalled(reason)),
                Err(MasksUnready::OutOfDate { stale }) => Err(MasksGap::OutOfDate { stale }),
                Err(MasksUnready::Shadowed) => Err(MasksGap::Shadowed),
                Err(MasksUnready::Unknown(reason)) => Err(MasksGap::Unknown(reason)),
                Err(MasksUnready::NotSetUp(reason)) => Err(MasksGap::NotSetUp {
                    reason,
                    setup: setup().await,
                }),
            },
            // A recorded engine the reconciler has not reached yet: within 2 s it starts.
            egress: host
                .proxies
                .state(project)
                .unwrap_or(cockpit_readiness::EngineEgress::Starting),
        },
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

/// `project` and the engine the owner recorded for it, if any.
async fn project_and_record(
    database: &Database,
    project: &str,
) -> Result<(Project, Option<RecordedEngine>), String> {
    let read = database.read().await.map_err(|error| error.to_string())?;
    let found = cockpit_projects::project_named(&read, project)
        .await
        .map_err(|error| error.to_string())?;
    let recorded = cockpit_containers::recorded_engine(&read, project)
        .await
        .map_err(|error| error.to_string())?;
    Ok((found, recorded))
}

/// Finds the engine each sandboxed launch reaches at `DOCKER_HOST` (#390): its project's,
/// once that engine answers as the one the owner recorded and Torlek's masks helper has
/// shown it the launch's checkout masked.
pub(crate) struct LaunchEngines {
    pub(crate) database: Arc<Database>,
    pub(crate) host: Host,
}

impl cockpit_sessions::ProjectEngines for LaunchEngines {
    fn reachable<'asked>(
        &'asked self,
        project: &'asked str,
        checkout: &'asked Path,
    ) -> cockpit_sessions::EngineAnswer<'asked> {
        Box::pin(self.engine_socket(project, checkout))
    }
}

impl LaunchEngines {
    /// The engine is verified first: the helper is asked to mask only for the project's own.
    async fn engine_socket(&self, project: &str, checkout: &Path) -> Option<EngineSocket> {
        let (user, passwd, state) = match self.examined(project).await {
            Ok(examined) => examined,
            Err(reason) => {
                tracing::warn!(project, %reason, "the launch reaches no project engine");
                return None;
            }
        };
        let socket = socket_of(&user, &state, &passwd)?;
        if let Err(reason) = shown_masked(&self.host.masks, &user, checkout).await {
            tracing::warn!(project, %reason, "the launch reaches no project engine");
            return None;
        }
        Some(socket)
    }

    async fn examined(&self, project: &str) -> Result<(EngineUser, String, EngineState), String> {
        let (found, recorded) = project_and_record(&self.database, project).await?;
        let passwd = read_passwd().await?;
        let user = self.host.engine_user(&found)?;
        let state = examine(&self.host, &user, &passwd, recorded.as_ref()).await;
        Ok((user, passwd, state))
    }
}

/// `user`'s Docker socket, when its engine answers as the one the owner recorded and
/// `passwd` holds its account; readiness says why when it does not.
fn socket_of(user: &EngineUser, state: &EngineState, passwd: &str) -> Option<EngineSocket> {
    if !matches!(state, EngineState::Verified(_)) {
        return None;
    }
    let account = cockpit_containers::account_named(passwd, user.name())?;
    Some(EngineSocket {
        user: user.name().to_owned(),
        uid: account.uid,
        socket: user.socket(),
    })
}

/// Whether the helper showed `user`'s engine `checkout` through every denial of the session
/// baseline in it: otherwise the engine's containers could write the checkout's Git hooks or
/// read its `.env`. Each launch asks again, so an environment file made since the last is
/// masked too.
async fn shown_masked(
    helper: &MasksHelper,
    user: &EngineUser,
    checkout: &Path,
) -> Result<(), String> {
    helper
        .show_masked(user, checkout, MASKS_DEADLINE)
        .await
        .map_err(|reason| {
            format!(
                "the engine of {user} is not shown {} masked: {reason}",
                checkout.display()
            )
        })
}

/// The forward `session` asks for, of `port` of `project`'s engine (#390), once that engine
/// answers as the one the owner recorded: an agent asks for a port of a verified engine
/// only, and never for one in another session's port block (#1371).
pub(crate) async fn container_port(
    database: &Database,
    host: &Host,
    project: &str,
    session: &str,
    port: u16,
) -> Result<Forward, Failed> {
    let (found, recorded) = {
        let read = database.read().await?;
        let found = registered(&read, project).await?;
        let held = cockpit_sessions::live_port_blocks(&read)
            .await
            .map_err(|error| internal(&error))?;
        if let Some(holder) = cockpit_sessions::other_sessions_block(&held, project, session, port)
        {
            return Err(in_another_sessions_block(port, holder));
        }
        let recorded = cockpit_containers::recorded_engine(&read, project).await?;
        (found, recorded)
    };
    let passwd = read_passwd().await.map_err(|reason| internal(&reason))?;
    let user = engine_user(host, &found)?;
    let state = examine(host, &user, &passwd, recorded.as_ref()).await;
    if !matches!(state, EngineState::Verified(_)) {
        return Err(not_verified(&state, &user, &found, host));
    }
    let forward = Forward::container_port(user.name(), port);
    if let Some(problem) = forward.live_problem() {
        return Err(Failed(ProtocolError {
            code: ErrorCode::InvalidArgument,
            message: format!("port {port} cannot open in the sandbox: {problem}"),
            unblock: Some(Unblock {
                action: "publish the container on a port from 1024 up, not the session \
                         proxy's, and ask for that one"
                    .to_owned(),
                by: UnblockedBy::Agent,
            }),
        }));
    }
    Ok(forward)
}

fn in_another_sessions_block(port: u16, holder: &HeldPortBlock) -> Failed {
    Failed(ProtocolError {
        code: ErrorCode::InvalidArgument,
        message: format!(
            "port {port} is in session {}'s port block ({}), where only its containers publish",
            holder.session, holder.block
        ),
        unblock: Some(Unblock {
            action: format!(
                "publish your container on a port of your own block, ${PORT_BASE_VARIABLE} \
                 and up, and ask for that one"
            ),
            by: UnblockedBy::Agent,
        }),
    })
}

/// The project user's engine, as the decisions name it to the launcher (#390): the user the
/// owner recorded, its uid and its relay's socket. None when the owner recorded none. The
/// engine's identity was verified when the agent asked; the launcher checks the relay's
/// listener is that user at each connection.
pub(crate) async fn engine_relay(
    connection: &Connection,
    host: &Host,
    passwd: &str,
    project: &str,
) -> Result<Option<EngineRelay>, String> {
    let found = cockpit_projects::project_named(connection, project)
        .await
        .map_err(|error| error.to_string())?;
    let user = host.engine_user(&found)?;
    let recorded = cockpit_containers::recorded_engine(connection, project)
        .await
        .map_err(|error| error.to_string())?;
    Ok(relay_of(&user, recorded.as_ref(), passwd))
}

/// `user`'s relay, when the owner recorded that user's engine and `passwd` holds its account.
fn relay_of(
    user: &EngineUser,
    recorded: Option<&RecordedEngine>,
    passwd: &str,
) -> Option<EngineRelay> {
    if recorded?.user != user.name() {
        return None;
    }
    let account = cockpit_containers::account_named(passwd, user.name())?;
    Some(EngineRelay {
        user: user.name().to_owned(),
        uid: account.uid,
        socket: user.relay_socket(),
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

pub(crate) async fn read_passwd() -> Result<String, String> {
    tokio::fs::read_to_string(PASSWD_FILE)
        .await
        .map_err(|error| format!("cannot read {PASSWD_FILE}: {error}"))
}

fn engine_user(host: &Host, project: &Project) -> Result<EngineUser, Failed> {
    host.engine_user(project)
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

/// Why `project`'s engine serves no container port: it is not the one the owner recorded,
/// answering now. The owner's step fixes it; the agent can only ask again after.
fn not_verified(state: &EngineState, user: &EngineUser, project: &Project, host: &Host) -> Failed {
    let doctor = format!(
        "the owner runs `{} doctor --project {}` and the setup script it writes",
        host.qualified_profile,
        project.root.display()
    );
    let record = format!("the owner runs `{}`", record_command(host, project));
    let (problem, action) = match state {
        EngineState::NotSetUp { reason } => (reason.clone(), doctor),
        EngineState::Unrecorded(_) => ("the owner has not recorded it".to_owned(), record),
        EngineState::Refused(refusal) => (
            refusal.to_string(),
            match refusal.remedy() {
                Remedy::Start => format!("the owner runs `sudo systemctl start {}`", user.unit()),
                Remedy::Record => record,
                Remedy::Setup => doctor,
            },
        ),
        EngineState::Verified(_) => unreachable!("a verified engine serves container ports"),
    };
    Failed(ProtocolError {
        code: ErrorCode::Refused,
        message: format!(
            "project {}'s engine {user} is not verified, so no container port opens: {problem}",
            project.name
        ),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A helper that answers `answer` to every request, listening as this test's user.
    fn fake_masks_helper(answer: &'static str) -> (tempfile::TempDir, MasksHelper) {
        use std::os::unix::fs::MetadataExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("engine-masks.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = [0u8; 512];
                let _ = stream.read(&mut request).await;
                let _ = stream.write_all(answer.as_bytes()).await;
            }
        });
        let uid = std::fs::metadata(&socket).unwrap().uid();
        let engines_dir = dir.path().join("engines");
        let install_root = dir.path().join("root");
        (
            dir,
            MasksHelper {
                socket,
                uid,
                engines_dir,
                install_root,
            },
        )
    }

    /// #390: a launch reaches the engine only once the helper masked its checkout.
    #[tokio::test]
    async fn a_launch_reaches_the_engine_once_the_helper_masked_its_checkout() {
        let (_dir, helper) = fake_masks_helper("ok\n");
        let user = EngineUser::for_project("torlek", 3).unwrap();

        let shown = shown_masked(&helper, &user, Path::new("/home/owner/app")).await;

        assert_eq!(shown, Ok(()));
    }

    #[tokio::test]
    async fn a_checkout_the_helper_refuses_reaches_no_engine() {
        let (_dir, helper) = fake_masks_helper("refused: /home/owner/app holds no .git\n");
        let user = EngineUser::for_project("torlek", 3).unwrap();

        let refused = shown_masked(&helper, &user, Path::new("/home/owner/app"))
            .await
            .unwrap_err();

        assert!(refused.contains("holds no .git"), "{refused}");
    }

    #[tokio::test]
    async fn no_engine_is_reached_without_the_helper() {
        let dir = tempfile::tempdir().unwrap();
        let helper = MasksHelper {
            socket: dir.path().join("absent.sock"),
            uid: 0,
            engines_dir: dir.path().join("engines"),
            install_root: dir.path().to_path_buf(),
        };
        let user = EngineUser::for_project("torlek", 3).unwrap();

        let refused = shown_masked(&helper, &user, Path::new("/home/owner/app"))
            .await
            .unwrap_err();

        assert!(refused.contains("cannot connect"), "{refused}");
    }

    const PASSWD: &str = "root:x:0:0::/root:/bin/sh\n\
                          torlek-e3:x:991:991::/var/lib/torlek-e3:/usr/bin/nologin\n";

    fn recorded(user: &str) -> RecordedEngine {
        RecordedEngine {
            project: "app".to_owned(),
            user: user.to_owned(),
            engine_id: "engine-1".to_owned(),
            recorded_at: 0,
            recorded_through: None,
        }
    }

    #[test]
    fn a_recorded_engine_names_its_users_uid_and_relay() {
        let user = EngineUser::for_project("torlek", 3).unwrap();

        let relay = relay_of(&user, Some(&recorded("torlek-e3")), PASSWD);

        assert_eq!(
            relay,
            Some(EngineRelay {
                user: "torlek-e3".to_owned(),
                uid: 991,
                socket: "/run/torlek-e3/relay.sock".into(),
            })
        );
    }

    #[test]
    fn no_relay_without_the_projects_recorded_engine_and_its_account() {
        let user = EngineUser::for_project("torlek", 3).unwrap();
        let without_account = "root:x:0:0::/root:/bin/sh\n";

        for (case, engine, passwd) in [
            ("nothing recorded", None, PASSWD),
            ("another user recorded", Some(recorded("torlek-e4")), PASSWD),
            ("no account", Some(recorded("torlek-e3")), without_account),
        ] {
            assert_eq!(relay_of(&user, engine.as_ref(), passwd), None, "{case}");
        }
    }

    fn info() -> cockpit_containers::EngineInfo {
        cockpit_containers::EngineInfo {
            id: "engine-1".to_owned(),
            name: "box".to_owned(),
            server_version: "28.0.0".to_owned(),
            rootless: true,
            in_desktop_vm: false,
        }
    }

    #[test]
    fn a_verified_engine_names_its_users_uid_and_docker_socket() {
        let user = EngineUser::for_project("torlek", 3).unwrap();

        let socket = socket_of(&user, &EngineState::Verified(info()), PASSWD);

        assert_eq!(
            socket,
            Some(EngineSocket {
                user: "torlek-e3".to_owned(),
                uid: 991,
                socket: "/run/torlek-e3/docker.sock".into(),
            })
        );
    }

    #[test]
    fn no_docker_socket_for_an_engine_not_verified_or_without_its_account() {
        let user = EngineUser::for_project("torlek", 3).unwrap();
        let without_account = "root:x:0:0::/root:/bin/sh\n";

        for (case, state, passwd) in [
            ("unrecorded", EngineState::Unrecorded(info()), PASSWD),
            (
                "not set up",
                EngineState::NotSetUp {
                    reason: "no socket".to_owned(),
                },
                PASSWD,
            ),
            ("no account", EngineState::Verified(info()), without_account),
        ] {
            assert_eq!(socket_of(&user, &state, passwd), None, "{case}");
        }
    }
}
