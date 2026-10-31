#![doc = include_str!("../README.md")]

mod agent;
mod approvals;
mod error;
mod feed;
mod instance;
mod lanes;
mod projects;
mod resources;
mod routes;
mod server;
mod sessions;
mod supervisor;
mod tasks;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;

pub use approvals::{RUN_LEADER_FLAG, run_settings};
pub use error::StartError;
pub use instance::{Instance, Options};
pub use lanes::LaneLimits;
pub use routes::{Handler, Job, Peer, Route, Routes, StreamJob, WorkClass, refusal};
pub use server::Limits;

/// Every capability's migrations, in the order the daemon applies them.
pub const MIGRATION_SETS: [cockpit_turso::MigrationSet; 9] = [
    cockpit_authorization::MIGRATIONS,
    cockpit_agent_channel::MIGRATIONS,
    cockpit_projects::MIGRATIONS,
    cockpit_processes::MIGRATIONS,
    cockpit_approvals::MIGRATIONS,
    cockpit_sessions::MIGRATIONS,
    cockpit_messaging::MIGRATIONS,
    cockpit_tasks::MIGRATIONS,
    cockpit_resource_admission::MIGRATIONS,
];

/// How often expired and ended reservations are released.
const RESOURCE_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// What the daemon wires its capabilities with: the host's tools in the binary, fixtures in
/// tests.
pub struct Wiring {
    /// Cockpit's tmux server; opening it is the tmux readiness check.
    pub tmux: cockpit_tmux::ServerSpec,
    /// The host Git that project commands and clones run, with the safe settings applied.
    pub git: cockpit_projects::Git,
    /// The agent providers sessions may launch.
    pub providers: Vec<Arc<dyn cockpit_provider_api::Provider>>,
    pub sessions: cockpit_sessions::Settings,
    /// How often the sessions' periodic pass (exits, idle sleep, expiry) runs.
    pub session_tick: Duration,
    /// How approved commands run ([`run_settings`]).
    pub run_settings: cockpit_approvals::RunSettings,
}

/// The capabilities a started daemon serves, for the [`Handler`] its caller builds.
#[derive(Clone)]
pub struct Capabilities {
    pub database: Arc<cockpit_turso::Database>,
    pub git: cockpit_projects::Git,
    pub sessions: Arc<cockpit_sessions::Sessions>,
    pub runner: cockpit_approvals::Runner,
    /// What this host enforces, probed at start.
    pub admission: Arc<cockpit_resource_admission::Backend>,
    /// Stops the execution of a reservation (stop execution, report 10).
    pub processes: cockpit_processes::Supervisor,
}

/// A started daemon: it accepts requests until dropped or the process exits.
pub struct Daemon {
    socket_path: PathBuf,
    agent_socket_path: PathBuf,
    accepting: tokio::task::JoinHandle<()>,
    accepting_agents: tokio::task::JoinHandle<()>,
    ticking: tokio::task::JoinHandle<()>,
    retaining: tokio::task::JoinHandle<()>,
    sweeping: tokio::task::JoinHandle<()>,
    _database: Arc<cockpit_turso::Database>,
    _instance: Instance,
}

impl Instance {
    /// Runs the restart contract, then serves the handler `routes` builds over the
    /// capabilities: open the database (durable state loads with it), reconcile executions,
    /// restore the sessions that were active, and only then bind and accept (report 03).
    /// Nothing is dispatched before reconciliation and restore have finished.
    ///
    /// Two sockets share the handler and the lanes: the host-only socket, whose peers are
    /// the owner, and the agent socket the sandboxes reach, whose peers are only ever agents
    /// (report 17). Each has its own connection limit, so agents cannot take the owner's
    /// connections; agent requests share the bulk lane with the owner's bulk work.
    ///
    /// Approved commands run with `wiring.run_settings` through the runner `routes`
    /// receives, over the daemon's one supervisor of executions.
    pub async fn start(
        self,
        wiring: Wiring,
        routes: impl FnOnce(Capabilities) -> Arc<dyn Handler>,
    ) -> Result<Daemon, StartError> {
        let options = &self.options;
        let database = Arc::new(
            cockpit_turso::Database::open(&options.database_path(), &MIGRATION_SETS).await?,
        );
        tracing::info!(epoch = database.epoch(), "opened the state database");
        let processes = cockpit_processes::Supervisor::detect(
            options.executions_dir(),
            cockpit_processes::SystemdTools::from_path(),
        )
        .await;
        tracing::info!(
            containment = ?processes.launch_containment(),
            "new executions use this containment"
        );
        let runner = cockpit_approvals::Runner::new(wiring.run_settings, processes.clone());
        reconcile(&database, &processes, &runner).await?;
        let admission = Arc::new(
            cockpit_resource_admission::Backend::detect(
                cockpit_resource_admission::HostTools::from_host(
                    &cockpit_processes::SystemdTools::from_path(),
                ),
                processes.launch_containment(),
            )
            .await
            .map_err(StartError::Admission)?,
        );
        tracing::info!(
            platform = admission.platform().as_str(),
            "probed what this host enforces"
        );
        let stopper = processes.clone();
        // A missing or too old tmux leaves the daemon up with sessions refused (report 09);
        // any other failure to open the server is the daemon's own and stops the start.
        let tmux = match cockpit_tmux::Server::open(wiring.tmux) {
            Err(
                error @ (cockpit_tmux::Error::TmuxNotRunnable { .. }
                | cockpit_tmux::Error::UnrecognizedVersion { .. }
                | cockpit_tmux::Error::TmuxTooOld { .. }),
            ) => {
                tracing::warn!(%error, "tmux is unavailable: sessions are refused until it is installed and the daemon restarts");
                Err(error)
            }
            Err(error) => return Err(StartError::Tmux(error)),
            Ok(server) => Ok(server),
        };
        let sessions = Arc::new(cockpit_sessions::Sessions::new(
            Arc::clone(&database),
            processes,
            tmux,
            wiring.git.clone(),
            wiring.providers,
            wiring.sessions,
        ));
        sessions::restore(&sessions)
            .await
            .map_err(StartError::Restore)?;
        let socket_path = options.socket_path();
        let agent_socket_path = options.agent_socket_path();
        let listener = Arc::new(server::bind(&socket_path)?);
        let agent_listener = Arc::new(server::bind(&agent_socket_path)?);
        let limits = options.limits;
        let shared = Arc::new(server::Shared {
            owner_uid: self.owner_uid,
            limits,
            lanes: lanes::Lanes::new(limits.control, limits.bulk),
            handler: routes(Capabilities {
                database: Arc::clone(&database),
                git: wiring.git,
                sessions: Arc::clone(&sessions),
                runner,
                admission,
                processes: stopper,
            }),
        });
        let ticking = tokio::spawn(supervisor::supervise("sessions-tick", move || {
            sessions::tick_forever(Arc::clone(&sessions), wiring.session_tick)
        }));
        let retained = Arc::clone(&database);
        let retaining = tokio::spawn(supervisor::supervise("feed-retention", move || {
            feed::discard_history_forever(Arc::clone(&retained))
        }));
        let swept_database = Arc::clone(&database);
        let sweeping = tokio::spawn(supervisor::supervise("resources-sweep", move || {
            resources::sweep_forever(Arc::clone(&swept_database), RESOURCE_SWEEP_INTERVAL)
        }));
        let accepting = accept(&listener, &shared, server::Socket::HostOnly);
        let accepting_agents = accept(&agent_listener, &shared, server::Socket::Agent);
        tracing::info!(
            socket = %socket_path.display(),
            agent_socket = %agent_socket_path.display(),
            "accepting requests"
        );
        Ok(Daemon {
            socket_path,
            agent_socket_path,
            accepting,
            accepting_agents,
            ticking,
            retaining,
            sweeping,
            _database: database,
            _instance: self,
        })
    }
}

/// One supervised accept loop for `listener`, with its own connection limit.
fn accept(
    listener: &Arc<tokio::net::UnixListener>,
    shared: &Arc<server::Shared>,
    socket: server::Socket,
) -> tokio::task::JoinHandle<()> {
    let listener = Arc::clone(listener);
    let shared = Arc::clone(shared);
    let connections = Arc::new(Semaphore::new(shared.limits.connections));
    tokio::spawn(supervisor::supervise(socket.worker_name(), move || {
        server::accept_connections(
            Arc::clone(&listener),
            Arc::clone(&shared),
            Arc::clone(&connections),
            socket,
        )
    }))
}

/// Step 2 and 3 of the restart contract: each capability that holds launch intents,
/// reservations or execution identities reconciles them against the real processes here,
/// before the socket exists. Processes is the first, then approvals and messaging; sessions
/// restore after.
async fn reconcile(
    database: &Arc<cockpit_turso::Database>,
    processes: &cockpit_processes::Supervisor,
    runner: &cockpit_approvals::Runner,
) -> Result<(), StartError> {
    let reconciled = processes.reconcile(database).await?;
    for outcome in &reconciled {
        match outcome {
            cockpit_processes::Reconciled::Reattached(id) => {
                tracing::info!(execution = id.0, "reattached a running execution");
            }
            cockpit_processes::Reconciled::Ended(id, reason) => {
                tracing::info!(execution = id.0, ?reason, "released an ended execution");
            }
            cockpit_processes::Reconciled::Uncertain(id) => tracing::warn!(
                execution = id.0,
                "kept an unidentified execution charged; it may still be running"
            ),
            cockpit_processes::Reconciled::Unchecked(id, reason) => tracing::warn!(
                execution = id.0,
                reason,
                "kept an execution charged that could not be checked"
            ),
        }
    }
    tracing::info!(executions = reconciled.len(), "reconciled durable state");
    // Reservations follow their executions: one that ended while the daemon was down is
    // released before anything is admitted.
    resources::sweep(database)
        .await
        .map_err(StartError::Admission)?;
    approvals::reconcile_runs(database, runner)
        .await
        .map_err(StartError::Approvals)?;
    reconcile_deliveries(database).await
}

/// Deliveries the daemon stopped during: only a deduplicating receiver gets them again (#41).
async fn reconcile_deliveries(database: &cockpit_turso::Database) -> Result<(), StartError> {
    let write = database.write().await?;
    let reconciled = cockpit_messaging::reconcile_deliveries(&write, approvals::now())
        .await
        .map_err(StartError::Messaging)?;
    write.commit().await?;
    for (envelope, state) in reconciled {
        tracing::warn!(
            envelope = envelope.0,
            state = state.as_str(),
            "a delivery was in flight when the daemon stopped"
        );
    }
    Ok(())
}

impl Daemon {
    /// The host-only socket.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// The socket sandboxed agents reach.
    pub fn agent_socket_path(&self) -> &Path {
        &self.agent_socket_path
    }

    /// Serves until an accept loop ends, which only a cancelled runtime does.
    pub async fn wait(mut self) {
        tokio::select! {
            _ = &mut self.accepting => {}
            _ = &mut self.accepting_agents => {}
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.accepting.abort();
        self.accepting_agents.abort();
        self.ticking.abort();
        self.retaining.abort();
        self.sweeping.abort();
    }
}
