#![doc = include_str!("../README.md")]

mod agent;
mod approvals;
mod error;
mod instance;
mod lanes;
mod projects;
mod routes;
mod server;
mod supervisor;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Semaphore;

pub use approvals::{RUN_LEADER_FLAG, runner};
pub use error::StartError;
pub use instance::{Instance, Options};
pub use lanes::LaneLimits;
pub use routes::{Handler, Job, Peer, Route, Routes, WorkClass, refusal};
pub use server::Limits;

/// Every capability's migrations, in the order the daemon applies them.
pub const MIGRATION_SETS: [cockpit_turso::MigrationSet; 5] = [
    cockpit_authorization::MIGRATIONS,
    cockpit_agent_channel::MIGRATIONS,
    cockpit_projects::MIGRATIONS,
    cockpit_processes::MIGRATIONS,
    cockpit_approvals::MIGRATIONS,
];

/// A started daemon: it accepts requests until dropped or the process exits.
pub struct Daemon {
    socket_path: PathBuf,
    agent_socket_path: PathBuf,
    accepting: tokio::task::JoinHandle<()>,
    accepting_agents: tokio::task::JoinHandle<()>,
    _database: Arc<cockpit_turso::Database>,
    _instance: Instance,
}

impl Instance {
    /// Runs the restart contract, then serves the handler `routes` builds over the state
    /// database: open the database (durable state loads with it), reconcile, and only then
    /// bind and accept (report 03). Nothing is dispatched before reconciliation has finished.
    ///
    /// Two sockets share the handler and the lanes: the host-only socket, whose peers are
    /// the owner, and the agent socket the sandboxes reach, whose peers are only ever agents
    /// (report 17). Each has its own connection limit, so agents cannot take the owner's
    /// connections; agent requests share the bulk lane with the owner's bulk work.
    pub async fn start(
        self,
        routes: impl FnOnce(Arc<cockpit_turso::Database>) -> Arc<dyn Handler>,
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
        reconcile(&database, &processes).await?;
        let socket_path = options.socket_path();
        let agent_socket_path = options.agent_socket_path();
        let listener = Arc::new(server::bind(&socket_path)?);
        let agent_listener = Arc::new(server::bind(&agent_socket_path)?);
        let limits = options.limits;
        let shared = Arc::new(server::Shared {
            owner_uid: self.owner_uid,
            limits,
            lanes: lanes::Lanes::new(limits.control, limits.bulk),
            handler: routes(Arc::clone(&database)),
        });
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
/// before the socket exists. Processes is the first, then approvals; sessions follow in M1.
async fn reconcile(
    database: &cockpit_turso::Database,
    processes: &cockpit_processes::Supervisor,
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
    reconcile_runs(database).await
}

/// Approved runs the daemon stopped before or during: neither is repeated (#34).
async fn reconcile_runs(database: &cockpit_turso::Database) -> Result<(), StartError> {
    let write = database.write().await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs();
    let ended = cockpit_approvals::reconcile_runs(&write, i64::try_from(now).expect("fits"))
        .await
        .map_err(StartError::Approvals)?;
    write.commit().await?;
    for (request, state) in ended {
        tracing::warn!(
            request = request.0,
            state = state.as_str(),
            "an approved run did not finish before the restart"
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
    }
}
