#![doc = include_str!("../README.md")]

mod accounts;
mod agent;
mod agent_context;
mod approvals;
mod budgets;
mod bug_report_feed;
mod bug_reports;
mod clipboard;
mod coordination;
mod coordinator_workers;
mod cost;
mod credentials;
mod database_retention;
mod digest;
mod disk_guard;
mod end_sweep;
mod error;
mod feed;
mod instance;
mod lanes;
mod logs;
mod manager;
mod network;
mod output_retention;
mod projects;
mod quota_resume;
mod readiness;
mod resources;
mod routes;
mod scheduling;
mod serve;
mod server;
mod session_feed;
mod session_sockets;
mod sessions;
mod storage;
mod supervisor;
mod tasks;
mod turns;
mod usage;
mod verification;
mod versions;
mod view_stamp_feed;
mod workflows;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;

pub use agent_context::{DeliveredSkillPack, opencode_hidden_skills};
pub use approvals::{RUN_LEADER_FLAG, run_settings};
pub use cost::CostScanning;
pub use error::StartError;
pub use instance::{Instance, Options};
pub use lanes::{LaneLimits, StallBudgets};
pub use manager::Manager;
pub use routes::{BufferedBatch, Handler, Job, Peer, Route, Routes, StreamJob, WorkClass, refusal};
pub use serve::{EXIT_SWEEP, SESSION_TICK, Serve, own_profile, own_program, serve};
pub use server::Limits;
pub use session_sockets::SessionTreeSockets;
pub use usage::QuotaPolling;

/// Every capability's migrations, in the order the daemon applies them.
pub const MIGRATION_SETS: [cockpit_turso::MigrationSet; 15] = [
    cockpit_authorization::MIGRATIONS,
    cockpit_agent_channel::MIGRATIONS,
    cockpit_projects::MIGRATIONS,
    cockpit_processes::MIGRATIONS,
    cockpit_approvals::MIGRATIONS,
    cockpit_sessions::MIGRATIONS,
    cockpit_sandbox::MIGRATIONS,
    cockpit_messaging::MIGRATIONS,
    cockpit_tasks::MIGRATIONS,
    cockpit_resource_admission::MIGRATIONS,
    cockpit_workflows::MIGRATIONS,
    cockpit_verification::MIGRATIONS,
    cockpit_scheduling::MIGRATIONS,
    cockpit_bug_reports::MIGRATIONS,
    cockpit_usage::MIGRATIONS,
];

/// Names the directory holding the instruction bundle in every agent's environment.
pub const INSTRUCTIONS_VARIABLE: &str = "TORLEK_INSTRUCTIONS";

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
    /// The providers the start looked for and could not wire, each with why: the readiness
    /// report names them (#50).
    pub unavailable_providers: Vec<(String, cockpit_readiness::ProviderUnusable)>,
    pub sessions: cockpit_sessions::Settings,
    /// The owner's desktop session variables agents get until the owner imports others
    /// (#307): the daemon start's.
    pub desktop_environment: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    /// The program that reads the owner's desktop clipboard for CMD+V's image paste in the
    /// viewer (#848): `wl-paste`, found on `PATH` when it runs.
    pub clipboard_reader: std::path::PathBuf,
    /// How often the sessions' periodic pass (exits, idle sleep, expiry) runs.
    pub session_tick: Duration,
    /// How often the exits alone are looked at between ticks (#632): a dead agent shows as
    /// failed or ended within about this long. [`EXIT_SWEEP`] in the product. There is no sweep
    /// unless this is shorter than `session_tick`: a test whose passes are its own sets both to
    /// the hour, so a terminal it kills stays unnoticed.
    pub exit_sweep: Duration,
    /// How approved commands run ([`run_settings`]).
    pub run_settings: cockpit_approvals::RunSettings,
    /// The owner's environment, for the configuration the checkouts published on the change
    /// feed read (trusted projects).
    pub environment: cockpit_config::Environment,
    /// How waiting work is placed on sessions: the idle window, the pass interval, aging.
    pub scheduling: cockpit_scheduling::Settings,
    /// The bytes of change batches one subscriber may have waiting to be written (#406):
    /// the owner's `memory.feed_buffer_kib`.
    pub feed_buffer_bytes: u32,
    /// How each provider reporting `quota` is read (#58); `None` reads none, as in tests that
    /// are not about quota.
    pub quota: Option<QuotaPolling>,
    /// How each provider reporting `cost` is read for the cost archive (#699); `None` reads
    /// none, as in tests that are not about cost.
    pub cost: Option<CostScanning>,
    /// Each project's economy model and effort, by project name (#412): an item marked
    /// economy goes only to a session of its project started with them.
    pub economy: std::collections::BTreeMap<String, cockpit_scheduling::Economy>,
}

/// The sessions' settings the owner configured: the idle window and the asleep expiry
/// (report 09), and how long a session may run with no agent before it is reported (#406);
/// everything else keeps the product's values.
pub fn session_settings(config: &cockpit_config::EffectiveConfig) -> cockpit_sessions::Settings {
    cockpit_sessions::Settings {
        owner: owner_session_settings(config),
        agentless_after: Duration::from_secs(
            u64::from(config.memory.agentless_session_minutes.value) * 60,
        ),
        accounts: config.accounts.clone(),
        bundle: Some(versions::launch_bundle()),
        ..cockpit_sessions::Settings::default()
    }
}

/// The owner's per-project caps on each plan window; none for a daemon that reads no quota,
/// whose windows cap nothing (#412).
fn project_shares(quota: Option<&QuotaPolling>) -> std::collections::BTreeMap<String, u8> {
    quota
        .map(|polling| polling.project_shares.clone())
        .unwrap_or_default()
}

/// The owner's settings a running daemon holds until `config reload` (#354).
fn owner_session_settings(
    config: &cockpit_config::EffectiveConfig,
) -> cockpit_config::SessionSettings {
    cockpit_config::SessionSettings {
        idle_sleep_after: config.idle_sleep_after.value,
        asleep_expiry: config.asleep_expiry.value,
        desktop_session: config.agents_desktop_session.value,
        sandbox: config.agents_sandbox.value,
    }
}

/// The capabilities a started daemon serves, for the [`Handler`] its caller builds.
#[derive(Clone)]
pub struct Capabilities {
    /// Suppresses report-triggered relaunches until restore has finished.
    pub(crate) is_restoring: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) storage: storage::Context,
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
    /// What one change-feed subscriber may have waiting to be written (#406).
    pub feed_buffer_bytes: u32,
    /// The sandbox launches go through, for the readiness report (#50).
    pub sandbox: cockpit_sandbox::Sandbox,
    /// The tmux version the start found, or why it refused it.
    pub tmux: Result<String, String>,
    /// Every provider the start tried, wired or not, as readiness reports them.
    pub provider_facts: Vec<cockpit_readiness::ProviderFacts>,
    /// The adapters readiness asks for current sign-in facts (#443).
    pub sign_in_providers: Vec<Arc<dyn cockpit_provider_api::Provider>>,
    /// Notified when a coordinator queues a message, to wake its delivery (#410).
    pub messages_queued: Arc<tokio::sync::Notify>,
    /// Notified when the owner pauses or resumes the queue, to wake a scheduling pass (#646).
    pub queue_changed: Arc<tokio::sync::Notify>,
    /// Notified when the owner decides a network request or revokes a grant, to rewrite the
    /// launchers' decisions (#674).
    pub network_decided: Arc<tokio::sync::Notify>,
    /// What every bug report draft carries about this daemon (#427).
    pub bug_report_facts: Arc<cockpit_bug_reports::Facts>,
    /// `$XDG_STATE_HOME/cockpit`, where bug report bundles are written beside the logs.
    pub state_dir: PathBuf,
    /// The optional skill pack as this start delivered it (#786).
    pub skill_pack: Arc<DeliveredSkillPack>,
    /// The policy the quota worker judges readings under; `None` when it reads none (#58).
    pub quota_policy: Option<cockpit_usage::QuotaPolicy>,
    /// The owner's per-project caps on each plan window (#412); empty without quota.
    pub project_shares: std::collections::BTreeMap<String, u8>,
    /// Each project's economy model and effort (#412).
    pub economy: std::collections::BTreeMap<String, cockpit_scheduling::Economy>,
}

/// A started daemon: it accepts requests until dropped or the process exits.
pub struct Daemon {
    socket_path: PathBuf,
    agent_socket_path: PathBuf,
    accepting: tokio::task::JoinHandle<()>,
    accepting_agents: supervisor::AbortOnDrop,
    ticking: tokio::task::JoinHandle<()>,
    publishing: tokio::task::JoinHandle<()>,
    recording_turns: tokio::task::JoinHandle<()>,
    publishing_drafts: tokio::task::JoinHandle<()>,
    publishing_view_stamps: tokio::task::JoinHandle<()>,
    keeping_network: tokio::task::JoinHandle<()>,
    retaining: tokio::task::JoinHandle<()>,
    sweeping: tokio::task::JoinHandle<()>,
    dispatching: tokio::task::JoinHandle<()>,
    delivering: tokio::task::JoinHandle<()>,
    detecting: tokio::task::JoinHandle<()>,
    pruning: tokio::task::JoinHandle<()>,
    retiring_outputs: tokio::task::JoinHandle<()>,
    maintaining_database: tokio::task::JoinHandle<()>,
    sweeping_ended: tokio::task::JoinHandle<()>,
    guarding_disk: tokio::task::JoinHandle<()>,
    resuming_quota_stops: tokio::task::JoinHandle<()>,
    polling_quota: Option<tokio::task::JoinHandle<()>>,
    archiving_costs: Option<tokio::task::JoinHandle<()>>,
    prune_stop: Arc<std::sync::atomic::AtomicBool>,
    _database: Arc<cockpit_turso::Database>,
    _instance: Instance,
}

impl Instance {
    /// Runs the restart contract, then serves the handler `routes` builds over the
    /// capabilities: open the database (durable state loads with it), reconcile executions,
    /// serve agent startup reports while restoring active sessions, then accept owner
    /// commands and new dispatch (report 03).
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
        mut wiring: Wiring,
        routes: impl FnOnce(Capabilities) -> Arc<dyn Handler>,
    ) -> Result<Daemon, StartError> {
        let options = &self.options;
        // Rewritten at every start, so what agents read on disk matches this release (#49).
        let bundle_dir = agent_context::bundle_dir(&options.state_dir);
        let bundle = cockpit_agent_context::write_bundle(&bundle_dir)
            .map_err(StartError::InstructionBundle)?;
        tracing::info!(
            manifest = %bundle.id,
            supersedes = bundle.supersedes.as_deref().unwrap_or("none"),
            dir = %bundle_dir.display(),
            "wrote the agent instruction bundle"
        );
        let claude_skills = cockpit_claude::instruction_skills_dir(
            &agent_context::claude_skills_root(&options.state_dir),
        );
        cockpit_agent_context::write_bundle(&claude_skills)
            .map_err(StartError::InstructionBundle)?;
        let skill_pack = Arc::new(agent_context::deliver_skill_pack(
            &wiring.environment,
            &claude_skills,
            &bundle_dir,
        )?);
        wiring
            .sessions
            .base_environment
            .push((INSTRUCTIONS_VARIABLE.into(), bundle_dir.into_os_string()));
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
        {
            let write = database.write().await?;
            cockpit_projects::recover_target_prunes(&write)
                .await
                .map_err(StartError::Storage)?;
            write.commit().await?;
        }
        // One that fails stays retiring with its files, fenced from readers; the next start or
        // `storage outputs prune` retries it.
        match cockpit_processes::recover_output_retirements(&database, &processes).await {
            Ok(recovered) => {
                if recovered.finished > 0 {
                    tracing::info!(
                        finished = recovered.finished,
                        "finished interrupted output retirements"
                    );
                }
                for (execution, error) in recovered.failed {
                    tracing::warn!(execution = execution.0, %error, "output retirement unfinished");
                }
            }
            Err(error) => tracing::warn!(%error, "interrupted output retirements not checked"),
        }
        let storage_settings = storage::settings(wiring.environment.clone()).await?;
        let storage = storage::Context {
            database: Arc::clone(&database),
            git: wiring.git.clone(),
            policy: Arc::new(storage::prune_policy(&storage_settings)),
            sweeps_on_session_end: storage_settings.sweep_on_session_end.value,
            stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            supervisor: processes.clone(),
            outputs: Arc::new(output_retention::OutputPolicy::from_settings(
                &storage_settings,
            )),
            session_temps: options.session_temps(),
            logs: cockpit_diagnostics::logs_dir_in(&options.state_dir),
            sweep_now: Arc::new(tokio::sync::Notify::new()),
            retention: Arc::new(database_retention::DatabasePolicy::from_settings(
                &storage_settings,
                &options.state_dir,
            )),
        };
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
        // Refused when loaded, rather than at each launch, so readiness names the provider
        // as unusable from the start (#861).
        for refused in cockpit_sessions::refused_declarations(&wiring.providers) {
            tracing::error!(
                provider = %refused.provider,
                reason = %refused.reason,
                "provider not usable: its sandbox declaration is refused"
            );
            wiring
                .providers
                .retain(|provider| provider.name().as_str() != refused.provider);
            wiring.unavailable_providers.push((
                refused.provider,
                cockpit_readiness::ProviderUnusable::DeclarationRefused {
                    reason: refused.reason,
                },
            ));
        }
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
        let tmux_readiness = tmux
            .as_ref()
            .map(|server| server.version().to_string())
            .map_err(Clone::clone);
        let sandbox = wiring.sessions.sandbox.clone();
        let provider_facts: Vec<_> = wiring
            .providers
            .iter()
            .map(|provider| readiness::provider_found(provider.as_ref()))
            .chain(wiring.unavailable_providers.iter().map(|(name, unusable)| {
                cockpit_readiness::ProviderFacts {
                    name: name.clone(),
                    found: Err(unusable.clone()),
                }
            }))
            .collect();
        let manager = Arc::new(Manager::new(
            tmux.clone(),
            wiring.environment.clone(),
            wiring.clipboard_reader,
        ));
        let sign_in_providers = wiring.providers.clone();
        let quota_providers = wiring.providers.clone();
        let cost_providers = wiring.providers.clone();
        let sessions = Arc::new(cockpit_sessions::Sessions::new(
            Arc::clone(&database),
            processes,
            tmux,
            wiring.git.clone(),
            wiring.providers,
            cockpit_sessions::Settings {
                owner_environment: wiring.environment.clone(),
                ..wiring.sessions
            },
            options.session_dirs(),
            options.session_temps(),
            session_sockets::connect,
        ));
        sessions.import_desktop_environment(wiring.desktop_environment);
        let bug_report_facts = Arc::new(bug_reports::facts(
            sandbox.platform().name(),
            &tmux_readiness,
            &provider_facts,
        ));
        let (faults, fault_receiver) = bug_reports::Faults::channel();
        let socket_path = options.socket_path();
        let agent_socket_path = options.agent_socket_path();
        let agent_listener = Arc::new(server::bind(&agent_socket_path)?);
        let limits = options.limits;
        let messages_queued = Arc::new(tokio::sync::Notify::new());
        let queue_changed = Arc::new(tokio::sync::Notify::new());
        let network_decided = Arc::new(tokio::sync::Notify::new());
        let delivered = coordination::Context {
            database: Arc::clone(&database),
            sessions: Arc::clone(&sessions),
            runner: runner.clone(),
            queued: Arc::clone(&messages_queued),
            // The delivery loop never collects for a hook.
            waiters: Arc::new(coordination::Waiters::new()),
            // Nor starts a session.
            launching: Arc::default(),
        };
        let detected_readiness = readiness::Context {
            database: Arc::clone(&database),
            sessions: Arc::clone(&sessions),
            admission: Arc::clone(&admission),
            sandbox: sandbox.clone(),
            tmux: tmux_readiness.clone(),
            providers: provider_facts.clone(),
            sign_in_providers: sign_in_providers.clone(),
            environment: wiring.environment.clone(),
            manager: Arc::clone(&manager),
            early_answer: readiness::Context::early_answer(),
        };
        let is_restoring = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let shared = Arc::new(server::Shared {
            is_restoring: Arc::clone(&is_restoring),
            owner_uid: self.owner_uid,
            limits,
            lanes: lanes::Lanes::new(
                limits.control,
                limits.stop,
                limits.bulk,
                limits.stall_budgets,
                faults.clone(),
            ),
            handler: routes(Capabilities {
                is_restoring,
                storage: storage.clone(),
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
                feed_buffer_bytes: wiring.feed_buffer_bytes,
                sandbox,
                tmux: tmux_readiness,
                provider_facts,
                sign_in_providers,
                messages_queued,
                queue_changed: Arc::clone(&queue_changed),
                network_decided: Arc::clone(&network_decided),
                bug_report_facts: Arc::clone(&bug_report_facts),
                state_dir: options.state_dir.clone(),
                skill_pack,
                quota_policy: wiring.quota.as_ref().map(|polling| polling.policy.clone()),
                project_shares: project_shares(wiring.quota.as_ref()),
                economy: wiring.economy.clone(),
            }),
        });
        // Cancel the early listener if restore fails or startup is cancelled.
        let accepting_agents = supervisor::AbortOnDrop(accept(
            &agent_listener,
            &shared,
            server::Socket::Agent,
            &faults,
        ));
        sessions::restore(&sessions)
            .await
            .map_err(StartError::Restore)?;
        // Owner clients must fail to connect while restore is busy, not hang on a socket
        // whose accept loop has not started.
        let listener = Arc::new(server::bind(&socket_path)?);
        // Marked after the last step that can fail, so a refused start is not taken for a
        // crash at the next one: from here on only a `Daemon`, whose drop clears it, exists.
        let crashed = bug_reports::mark_running(&options.state_dir).map_err(StartError::io(
            "mark the daemon running in",
            &options.state_dir,
        ))?;
        if crashed {
            tracing::warn!("the last run did not stop cleanly; opening a bug report draft");
            faults.report(bug_reports::Fault::Crash);
        }
        shared
            .is_restoring
            .store(false, std::sync::atomic::Ordering::Release);
        let delivering = tokio::spawn(supervisor::supervise(
            "coordinator-messages",
            faults.clone(),
            move || coordination::deliver_forever(delivered.clone()),
        ));
        let detected = bug_reports::Context {
            database: Arc::clone(&database),
            state_dir: options.state_dir.clone(),
            facts: bug_report_facts,
            readiness: detected_readiness,
        };
        let fault_receiver = Arc::new(tokio::sync::Mutex::new(fault_receiver));
        // Its own panics are not faults it files: a fault that makes it panic would loop.
        let detecting = tokio::spawn(supervisor::supervise(
            "bug-reports",
            bug_reports::Faults::none(),
            move || bug_reports::detect_forever(Arc::clone(&fault_receiver), detected.clone()),
        ));
        let guard = disk_guard::DiskGuard {
            backend: Arc::clone(&admission),
            database: Arc::clone(&database),
            read_free_mib: cockpit_projects::free_space_mib,
            watched_path: options.state_dir.clone(),
            floor_mib: storage_settings.low_disk_floor_mib.value,
            queue_changed: Arc::clone(&queue_changed),
            sweep_now: Arc::clone(&storage.sweep_now),
            prune_automatic: storage.policy.automatic,
        };
        // Read once before any scheduling pass, so a restart on a full disk holds work at once.
        guard.check(&mut disk_guard::Memory::default()).await;
        let scheduled = scheduling::Context {
            database: Arc::clone(&database),
            sessions: Arc::clone(&sessions),
            admission: Arc::clone(&admission),
            manager,
            provider_capabilities: Arc::new(provider_capabilities),
            settings: wiring.scheduling,
            quota_policy: wiring.quota.as_ref().map(|polling| polling.policy.clone()),
            project_shares: project_shares(wiring.quota.as_ref()),
            economy: wiring.economy,
            queue_changed,
        };
        let dispatching = tokio::spawn(supervisor::supervise(
            "scheduling",
            faults.clone(),
            move || scheduling::dispatch_forever(scheduled.clone()),
        ));
        let published_database = Arc::clone(&database);
        let published_sessions = Arc::clone(&sessions);
        let environment = wiring.environment;
        let publishing = tokio::spawn(supervisor::supervise(
            "session-feed",
            faults.clone(),
            move || {
                session_feed::publish_forever(
                    Arc::clone(&published_database),
                    Arc::clone(&published_sessions),
                    environment.clone(),
                )
            },
        ));
        let turns_database = Arc::clone(&database);
        let turns_sessions = Arc::clone(&sessions);
        let recording_turns = tokio::spawn(supervisor::supervise(
            "turn-time",
            faults.clone(),
            move || turns::track_forever(Arc::clone(&turns_database), Arc::clone(&turns_sessions)),
        ));
        let drafts_database = Arc::clone(&database);
        let publishing_drafts = tokio::spawn(supervisor::supervise(
            "bug-report-feed",
            faults.clone(),
            move || bug_report_feed::publish_forever(Arc::clone(&drafts_database)),
        ));
        let stamped_database = Arc::clone(&database);
        let publishing_view_stamps = tokio::spawn(supervisor::supervise(
            "view-stamp-feed",
            faults.clone(),
            move || view_stamp_feed::publish_forever(Arc::clone(&stamped_database)),
        ));
        let network_database = Arc::clone(&database);
        let network_sandbox = sessions.sandbox().clone();
        let keeping_network = tokio::spawn(supervisor::supervise(
            "network-requests",
            faults.clone(),
            move || {
                network::keep_forever(
                    Arc::clone(&network_database),
                    network_sandbox.clone(),
                    Arc::clone(&network_decided),
                )
            },
        ));
        let resumed_database = Arc::clone(&database);
        let resumed_sessions = Arc::clone(&sessions);
        let resume_readings = wiring.quota.as_ref().map(|polling| quota_resume::Readings {
            policy: polling.policy.clone(),
            providers: quota_providers
                .iter()
                .filter(|provider| {
                    provider
                        .capabilities()
                        .supports(cockpit_provider_api::Capability::Quota)
                })
                .map(|provider| provider.name().as_str().to_owned())
                .collect(),
        });
        let resuming_quota_stops = tokio::spawn(supervisor::supervise(
            "quota-resume",
            faults.clone(),
            move || {
                quota_resume::resume_forever(
                    Arc::clone(&resumed_database),
                    Arc::clone(&resumed_sessions),
                    resume_readings.clone(),
                    wiring.session_tick,
                )
            },
        ));
        let scanned_sessions = Arc::clone(&sessions);
        let ticking = tokio::spawn(supervisor::supervise(
            "sessions-tick",
            faults.clone(),
            move || {
                sessions::tick_forever(
                    Arc::clone(&sessions),
                    wiring.session_tick,
                    wiring.exit_sweep,
                )
            },
        ));
        let retained = Arc::clone(&database);
        let feed_changes_kept = storage.retention.feed_changes_kept;
        let retaining = tokio::spawn(supervisor::supervise(
            "feed-retention",
            faults.clone(),
            move || feed::discard_history_forever(Arc::clone(&retained), feed_changes_kept),
        ));
        let swept_database = Arc::clone(&database);
        let sweeping = tokio::spawn(supervisor::supervise(
            "resources-sweep",
            faults.clone(),
            move || {
                resources::sweep_forever(
                    Arc::clone(&swept_database),
                    sweeper.clone(),
                    RESOURCE_SWEEP_INTERVAL,
                )
            },
        ));
        let accepting = accept(&listener, &shared, server::Socket::HostOnly, &faults);
        let outputs_shared = Arc::clone(&shared);
        let outputs_context = storage.clone();
        let retiring_outputs = tokio::spawn(supervisor::supervise(
            "output-retention",
            faults.clone(),
            move || {
                output_retention::maintain_forever(
                    outputs_context.clone(),
                    Arc::clone(&outputs_shared),
                )
            },
        ));
        let maintained_shared = Arc::clone(&shared);
        let maintained_context = storage.clone();
        let maintained_disk = Arc::clone(&admission);
        let maintaining_database = tokio::spawn(supervisor::supervise(
            "database-retention",
            faults.clone(),
            move || {
                database_retention::maintain_forever(
                    maintained_context.clone(),
                    Arc::clone(&maintained_shared),
                    Arc::clone(&maintained_disk),
                )
            },
        ));
        let end_sweep_storage = storage.clone();
        let storage_shared = Arc::clone(&shared);
        let prune_stop = Arc::clone(&storage.stopping);
        let pruning = tokio::spawn(supervisor::supervise(
            "target-pruning",
            faults.clone(),
            move || storage::maintain_forever(storage.clone(), Arc::clone(&storage_shared)),
        ));
        let end_sweep_shared = Arc::clone(&shared);
        let sweeping_ended = tokio::spawn(supervisor::supervise(
            "clone-end-sweep",
            faults.clone(),
            move || {
                end_sweep::sweep_forever(
                    end_sweep_storage.clone(),
                    Arc::clone(&end_sweep_shared),
                    wiring.session_tick,
                )
            },
        ));
        let guarding_disk = tokio::spawn(supervisor::supervise(
            "disk-guard",
            faults.clone(),
            move || disk_guard::guard_forever(guard.clone()),
        ));
        let polling_quota = wiring.quota.map(|polling| {
            let polled_database = Arc::clone(&database);
            tokio::spawn(supervisor::supervise(
                "quota-polling",
                faults.clone(),
                move || {
                    usage::poll_forever(
                        Arc::clone(&polled_database),
                        quota_providers.clone(),
                        polling.clone(),
                    )
                },
            ))
        });
        let archiving_costs = wiring.cost.map(|scanning| {
            let archivist = Arc::new(cost::Archivist::new(
                Arc::clone(&database),
                cost_providers,
                &scanning,
            ));
            scanned_sessions.observe_homes(archivist.clone());
            tokio::spawn(supervisor::supervise(
                "cost-archive",
                faults.clone(),
                move || {
                    Arc::clone(&archivist)
                        .scan_forever(Arc::clone(&scanned_sessions), scanning.clone())
                },
            ))
        });
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
            recording_turns,
            publishing_drafts,
            publishing_view_stamps,
            keeping_network,
            retaining,
            sweeping,
            dispatching,
            delivering,
            detecting,
            pruning,
            retiring_outputs,
            maintaining_database,
            sweeping_ended,
            guarding_disk,
            resuming_quota_stops,
            polling_quota,
            archiving_costs,
            prune_stop,
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
    faults: &bug_reports::Faults,
) -> tokio::task::JoinHandle<()> {
    let listener = Arc::clone(listener);
    let shared = Arc::clone(shared);
    let connections = Arc::new(Semaphore::new(shared.limits.connections));
    tokio::spawn(supervisor::supervise(
        socket.worker_name(),
        faults.clone(),
        move || {
            server::accept_connections(
                Arc::clone(&listener),
                Arc::clone(&shared),
                Arc::clone(&connections),
                socket,
            )
        },
    ))
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
            _ = &mut self.accepting_agents.0 => {}
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.prune_stop
            .store(true, std::sync::atomic::Ordering::Release);
        self.accepting.abort();
        self.accepting_agents.0.abort();
        self.ticking.abort();
        self.publishing.abort();
        self.recording_turns.abort();
        self.publishing_drafts.abort();
        self.publishing_view_stamps.abort();
        self.keeping_network.abort();
        self.retaining.abort();
        self.sweeping.abort();
        self.dispatching.abort();
        self.delivering.abort();
        self.detecting.abort();
        self.pruning.abort();
        self.retiring_outputs.abort();
        self.maintaining_database.abort();
        self.sweeping_ended.abort();
        self.guarding_disk.abort();
        self.resuming_quota_stops.abort();
        if let Some(polling) = &self.polling_quota {
            polling.abort();
        }
        if let Some(archiving) = &self.archiving_costs {
            archiving.abort();
        }
        bug_reports::mark_stopped(&self._instance.options.state_dir);
    }
}
