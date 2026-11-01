#![doc = include_str!("../README.md")]

mod agent;
mod approvals;
mod error;
mod feed;
mod instance;
mod lanes;
mod manager;
mod projects;
mod resources;
mod routes;
mod scheduling;
mod serve;
mod server;
mod session_feed;
mod sessions;
mod supervisor;
mod tasks;
mod verification;
mod workflows;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;

pub use approvals::{RUN_LEADER_FLAG, run_settings};
pub use error::StartError;
pub use instance::{Instance, Options};
pub use lanes::LaneLimits;
pub use manager::Manager;
pub use routes::{Handler, Job, Peer, Route, Routes, StreamJob, WorkClass, refusal};
pub use serve::{SESSION_TICK, Serve, own_program, serve};
pub use server::Limits;

/// Every capability's migrations, in the order the daemon applies them.
pub const MIGRATION_SETS: [cockpit_turso::MigrationSet; 12] = [
    cockpit_authorization::MIGRATIONS,
    cockpit_agent_channel::MIGRATIONS,
    cockpit_projects::MIGRATIONS,
    cockpit_processes::MIGRATIONS,
    cockpit_approvals::MIGRATIONS,
    cockpit_sessions::MIGRATIONS,
    cockpit_messaging::MIGRATIONS,
    cockpit_tasks::MIGRATIONS,
    cockpit_resource_admission::MIGRATIONS,
    cockpit_workflows::MIGRATIONS,
    cockpit_verification::MIGRATIONS,
    cockpit_scheduling::MIGRATIONS,
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
    /// The owner's desktop session variables agents get until the owner imports others
    /// (#307): the daemon start's.
    pub desktop_environment: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    /// How often the sessions' periodic pass (exits, idle sleep, expiry) runs.
    pub session_tick: Duration,
    /// How approved commands run ([`run_settings`]).
    pub run_settings: cockpit_approvals::RunSettings,
    /// The owner's environment, for the configuration the checkouts published on the change
    /// feed read (trusted projects).
    pub environment: cockpit_config::Environment,
    /// How waiting work is placed on sessions: the idle window, the pass interval, aging.
    pub scheduling: cockpit_scheduling::Settings,
}

/// The sessions' settings the owner configured, the idle window and the asleep expiry
/// (report 09); everything else keeps the product's values.
pub fn session_settings(config: &cockpit_config::EffectiveConfig) -> cockpit_sessions::Settings {
    cockpit_sessions::Settings {
        owner: owner_session_settings(config),
        ..cockpit_sessions::Settings::default()
    }
}

/// The owner's settings a running daemon holds until `config reload` (#354).
fn owner_session_settings(
    config: &cockpit_config::EffectiveConfig,
) -> cockpit_config::SessionSettings {
    cockpit_config::SessionSettings {
        idle_sleep_after: config.idle_sleep_after.value,
        asleep_expiry: config.asleep_expiry.value,
        desktop_session: config.agents_desktop_session.value,
    }
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
    /// The manager tmux session's handle on the same private server.
    pub manager: Arc<Manager>,
    /// The providers sessions may launch, as `provider-list` answers them.
    pub providers: Vec<cockpit_protocol::ProviderView>,
    /// What each provider reports it can do, by name: a workflow step checks it before a
    /// session begins it.
    pub provider_capabilities:
        std::collections::BTreeMap<String, cockpit_provider_api::Capabilities>,
    /// How approved commands run: a check the owner runs gets the same shell and environment.
    pub run_settings: Arc<cockpit_approvals::RunSettings>,
    pub scheduling: cockpit_scheduling::Settings,
}

/// A started daemon: it accepts requests until dropped or the process exits.
pub struct Daemon {
    socket_path: PathBuf,
    agent_socket_path: PathBuf,
    accepting: tokio::task::JoinHandle<()>,
    accepting_agents: tokio::task::JoinHandle<()>,
    ticking: tokio::task::JoinHandle<()>,
    publishing: tokio::task::JoinHandle<()>,
    retaining: tokio::task::JoinHandle<()>,
    sweeping: tokio::task::JoinHandle<()>,
    dispatching: tokio::task::JoinHandle<()>,
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
        let mut processes = cockpit_processes::Supervisor::detect(
            options.executions_dir(),
            cockpit_processes::SystemdTools::from_path(),
        )
        .await;
        // An agent's program waits from its start through the launch's confirm and the step
        // that records its root; a launch slower than that has already failed (#316).
        processes.release_window = processes.launch_timeout + 2 * wiring.sessions.step_timeout;
        tracing::info!(
            containment = ?processes.launch_containment(),
            "new executions use this containment"
        );
        let run_settings = Arc::new(wiring.run_settings.clone());
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
        let sweeper = processes.clone();
        let provider_capabilities: std::collections::BTreeMap<_, _> = wiring
            .providers
            .iter()
            .map(|provider| (provider.name().as_str().to_owned(), provider.capabilities()))
            .collect();
        let providers = wiring
            .providers
            .iter()
            .map(|provider| cockpit_protocol::ProviderView {
                name: provider.name().as_str().to_owned(),
                resume: provider
                    .capabilities()
                    .supports(cockpit_provider_api::Capability::Resume),
                events: provider
                    .capabilities()
                    .supports(cockpit_provider_api::Capability::Events),
            })
            .collect();
        // Opened once: the sessions and the manager share the handle, so one config write and
        // one version check decide for both. A missing or too old tmux leaves the daemon up
        // with sessions and the manager refused (report 09); any other failure to open the
        // server is the daemon's own and stops the start.
        let tmux = match cockpit_tmux::Server::open(wiring.tmux) {
            Err(
                error @ (cockpit_tmux::Error::TmuxNotRunnable { .. }
                | cockpit_tmux::Error::UnrecognizedVersion { .. }
                | cockpit_tmux::Error::TmuxTooOld { .. }),
            ) => {
                tracing::warn!(%error, "tmux is unavailable: sessions and the manager are refused until it is installed and the daemon restarts");
                Err(error.to_string())
            }
            Err(error) => return Err(StartError::Tmux(error)),
            Ok(server) => Ok(Arc::new(server)),
        };
        let manager = Arc::new(Manager::new(tmux.clone()));
        let sessions = Arc::new(cockpit_sessions::Sessions::new(
            Arc::clone(&database),
            processes,
            tmux,
            wiring.git.clone(),
            wiring.providers,
            wiring.sessions,
        ));
        sessions.import_desktop_environment(wiring.desktop_environment);
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
            lanes: lanes::Lanes::new(limits.control, limits.stop, limits.bulk),
            handler: routes(Capabilities {
                database: Arc::clone(&database),
                git: wiring.git,
                sessions: Arc::clone(&sessions),
                runner,
                admission: Arc::clone(&admission),
                processes: stopper,
                manager: Arc::clone(&manager),
                providers,
                provider_capabilities: provider_capabilities.clone(),
                run_settings,
                scheduling: wiring.scheduling,
            }),
        });
        let scheduled = scheduling::Context {
            database: Arc::clone(&database),
            sessions: Arc::clone(&sessions),
            admission: Arc::clone(&admission),
            manager,
            provider_capabilities: Arc::new(provider_capabilities),
            settings: wiring.scheduling,
        };
        let dispatching = tokio::spawn(supervisor::supervise("scheduling", move || {
            scheduling::dispatch_forever(scheduled.clone())
        }));
        let published_database = Arc::clone(&database);
        let published_sessions = Arc::clone(&sessions);
        let environment = wiring.environment;
        let publishing = tokio::spawn(supervisor::supervise("session-feed", move || {
            session_feed::publish_forever(
                Arc::clone(&published_database),
                Arc::clone(&published_sessions),
                environment.clone(),
            )
        }));
        let ticking = tokio::spawn(supervisor::supervise("sessions-tick", move || {
            sessions::tick_forever(Arc::clone(&sessions), wiring.session_tick)
        }));
        let retained = Arc::clone(&database);
        let retaining = tokio::spawn(supervisor::supervise("feed-retention", move || {
            feed::discard_history_forever(Arc::clone(&retained))
        }));
        let swept_database = Arc::clone(&database);
        let sweeping = tokio::spawn(supervisor::supervise("resources-sweep", move || {
            resources::sweep_forever(
                Arc::clone(&swept_database),
                sweeper.clone(),
                RESOURCE_SWEEP_INTERVAL,
            )
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
            publishing,
            retaining,
            sweeping,
            dispatching,
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
    resources::sweep(database, processes)
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
        self.publishing.abort();
        self.retaining.abort();
        self.sweeping.abort();
        self.dispatching.abort();
    }
}
