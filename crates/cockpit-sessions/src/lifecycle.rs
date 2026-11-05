//! The lifecycle operations: every transition goes through the stored state first, so two
//! operations on one session cannot interleave and a daemon that stopped midway finishes or
//! repairs the transition at its next start ([`Sessions::restore`]).

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cockpit_authorization::{Permission, Scope};
use cockpit_config::StartFileSnapshot;
use cockpit_processes::{ExecutionId, LaunchSpec, Phase, Supervisor, Termination};
use cockpit_projects::{Checkout, CheckoutKind, Git, Holder};
use cockpit_provider_api::{
    Capability, Conversation, LaunchRequest, Provider, ProviderError, TrustRequest, WorkspaceTrust,
};
use cockpit_tmux::{PaneId, PaneInfo, PaneState, Server, SessionLaunch, SessionName};
use cockpit_turso::Database;

#[path = "messaging.rs"]
mod messaging;
pub use messaging::visible_line;

use crate::model::{LaunchPurpose, validate_link};
use crate::store::{self, NewSession};
use crate::{
    AgentExit, AgentState, Ender, Error, Launch, LaunchKind, QuestionSource, RestartAll, Restored,
    Session, SessionId, SessionReport, SessionState, Settings, SleepRefusal, StartRequest,
    StopTarget, TickReport, WakeCause,
};

/// The variable that hands the agent its one-time launch token (report 17).
const LAUNCH_TOKEN_VARIABLE: &str = "TORLEK_LAUNCH_TOKEN";

/// The session a dry run's launch identity names: no session launches under it.
const DRY_RUN_SESSION: &str = "dry-run";

/// How often [`Sessions::after_early_exit`] looks at a new agent within its window.
const EARLY_EXIT_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Variables of the terminal tmux emulates, kept from the pane: they describe the host.
const TERMINAL_VARIABLES: [&str; 2] = ["TERM", "COLORTERM"];

/// What every launch and every restore of a still-running session grants the session over
/// itself, and every move to `ended` or `failed` revokes: reporting its own state (#143),
/// and asking the owner to run a command or follow a guide (#270), which the owner still
/// approves one by one.
const SELF_PERMISSIONS: [Permission; 2] = [Permission::ChangeState, Permission::RunCommands];

/// The states the owner's restart starts from: running, and the transitional states an
/// error or a crash can leave, in which the agent's status is unknown.
const RESTARTABLE: [SessionState; 5] = [
    SessionState::Running,
    SessionState::Launching,
    SessionState::Stopping {
        to: StopTarget::Asleep,
    },
    SessionState::Stopping {
        to: StopTarget::Ended,
    },
    SessionState::Stopping {
        to: StopTarget::Restart,
    },
];

/// The sessions capability over the state database, the process supervisor, Cockpit's
/// tmux server and the available providers.
pub struct Sessions {
    database: Arc<Database>,
    connect_socket: crate::ConnectSessionSocket,
    processes: Supervisor,
    /// Cockpit's tmux server, which the caller opens and may share, or why it could not be
    /// opened: the daemon starts without tmux (report 09), and each operation that needs a
    /// terminal refuses with that reason.
    tmux: Result<Arc<Server>, String>,
    git: Git,
    providers: Vec<Arc<dyn Provider>>,
    settings: Settings,
    /// Where each session gets its own directory, `<session dirs>/<session id>`, which its
    /// provider's launch and delivery use ([`LaunchRequest::session_dir`]).
    session_dirs: PathBuf,
    /// Where each session gets its own temporary folder, `<session temps>/<session id>`, on
    /// the main disk: its agent's `TMPDIR`, never the host's shared `/tmp` (#766).
    session_temps: PathBuf,
    /// One lock per session, held by each owner operation and each periodic step on it, so
    /// the periodic pass never acts on a launch an owner operation just replaced. An entry
    /// nothing holds carries no state; the periodic pass drops those (#406), so the map
    /// holds the sessions being worked on, not every session ever touched.
    operations: Mutex<HashMap<SessionId, Arc<SessionLock>>>,
    /// The running sessions whose agent the periodic pass has seen gone at every pass since
    /// (#406). One the latest pass did not see so is dropped; a daemon restart counts again
    /// from its start.
    agentless: Mutex<HashMap<SessionId, Agentless>>,
    /// The owner's desktop session variables the next launch gets: none until the daemon
    /// imports its start's, then each owner import (#307).
    desktop_environment: Mutex<Vec<(OsString, OsString)>>,
    /// One permit per outstanding blocking step, held by its thread until the step returns,
    /// not until its caller stops waiting (report 03: "cancellation has two facts"), so hung
    /// steps cannot take the whole blocking pool (#325).
    blocking_permits: Arc<tokio::sync::Semaphore>,
    /// The owner's session settings in force: `settings`' at start, then each
    /// [`Sessions::apply_owner_settings`] (#354). Read once per use, so one launch or one
    /// pass sees one value.
    owner_settings: Mutex<cockpit_config::SessionSettings>,
}

/// A session's operation lock, and whether a launch holds it. A stop never
/// waits behind a launch (#230, report 03): it holds a stop-lane worker while it waits, and
/// a launch may wait minutes on its provider, so a stop is refused while a launch runs.
#[derive(Default)]
struct SessionLock {
    operation: Arc<tokio::sync::Mutex<()>>,
    /// The live [`Launching`] marks; nested ones (a restart's stop, then its launch) count,
    /// so the inner one ending does not unmark the outer.
    launches: AtomicUsize,
    /// Wakes the stops waiting for the lock when a launch starts, so they refuse at once
    /// instead of waiting behind it.
    launch_started: tokio::sync::Notify,
    /// How many steps that may close or open the session's terminal have not returned,
    /// timed-out ones included. A launch opens the terminal only at zero (#324): the terminal
    /// is named by the session alone, so a stale step would act on the new launch's.
    terminal_steps: tokio::sync::watch::Sender<usize>,
}

/// A running session seen with its agent gone while its execution is occupied (#406).
struct Agentless {
    /// The launch whose agent is gone; another launch counts from its own first pass.
    launch: i64,
    /// Seconds since 1970 of the first pass that saw it.
    since: i64,
    /// The owner was told, so later passes write nothing more.
    is_reported: bool,
}

/// Whether anything but the sessions' own map holds `session`.
fn session_lock_is_held(session: &Arc<SessionLock>) -> bool {
    Arc::strong_count(session) > 1 || Arc::strong_count(&session.operation) > 1
}

/// A step counted in [`SessionLock::terminal_steps`], moved into its thread so it counts
/// until the step returns.
struct TerminalStep(Arc<SessionLock>);

impl TerminalStep {
    fn start(session: &Arc<SessionLock>) -> TerminalStep {
        session.terminal_steps.send_modify(|steps| *steps += 1);
        TerminalStep(Arc::clone(session))
    }
}

impl Drop for TerminalStep {
    fn drop(&mut self) {
        self.0.terminal_steps.send_modify(|steps| *steps -= 1);
    }
}

/// A held session lock: every owner operation and periodic step holds one. Only a launch
/// marks it, through [`Operation::launching`]: [`Sessions::launch`] and a restart's stop
/// take the held lock and mark it for their whole duration, so no launch runs unmarked.
struct Operation {
    _guard: tokio::sync::OwnedMutexGuard<()>,
    session: Arc<SessionLock>,
}

impl Operation {
    fn launching(&self) -> Launching<'_> {
        self.session.launches.fetch_add(1, Ordering::SeqCst);
        self.session.launch_started.notify_waiters();
        Launching(&self.session)
    }
}

/// A launch, or a restart's stop and relaunch, in progress on a session; dropped when it
/// returns.
struct Launching<'lock>(&'lock SessionLock);

impl Drop for Launching<'_> {
    fn drop(&mut self) {
        self.0.launches.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Sessions {
    #[allow(
        clippy::too_many_arguments,
        reason = "capabilities and the daemon-owned socket boundary"
    )]
    pub fn new(
        database: Arc<Database>,
        processes: Supervisor,
        tmux: Result<Arc<Server>, String>,
        git: Git,
        providers: Vec<Arc<dyn Provider>>,
        settings: Settings,
        session_dirs: PathBuf,
        session_temps: PathBuf,
        connect_socket: crate::ConnectSessionSocket,
    ) -> Sessions {
        assert!(
            settings.blocking_steps > 0,
            "the sessions need at least one blocking step permit"
        );
        Sessions {
            blocking_permits: Arc::new(tokio::sync::Semaphore::new(settings.blocking_steps)),
            database,
            connect_socket,
            processes,
            tmux,
            git,
            providers,
            desktop_environment: Mutex::new(Vec::new()),
            owner_settings: Mutex::new(settings.owner),
            settings,
            session_dirs,
            session_temps,
            operations: Mutex::new(HashMap::new()),
            agentless: Mutex::new(HashMap::new()),
        }
    }

    /// Replaces the owner's desktop session variables every later launch gets (#307). The
    /// whole set: a variable missing from `variables` is unset in later agents. Running
    /// agents keep the values they started with.
    pub fn import_desktop_environment(&self, variables: Vec<(OsString, OsString)>) {
        *self
            .desktop_environment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = variables;
    }

    /// The owner's desktop session variables last imported, whether or not agents get them:
    /// the daemon reads the owner's clipboard with them (#848).
    pub fn desktop_environment(&self) -> Vec<(OsString, OsString)> {
        self.desktop_environment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Whether launched agents get the owner's desktop session variables (#305).
    pub fn desktop_session(&self) -> crate::DesktopSession {
        self.owner_settings().desktop_session
    }

    /// The sandbox every launch goes through, with the backend the daemon probed.
    pub fn sandbox(&self) -> &cockpit_sandbox::Sandbox {
        &self.settings.sandbox
    }

    /// The owner's session settings in force now.
    pub fn owner_settings(&self) -> cockpit_config::SessionSettings {
        *self
            .owner_settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Replaces the owner's session settings for every later launch and sessions pass, and
    /// returns the ones they replace (#354). Running agents keep the environment they
    /// started with; a sleeping session's expiry and an idle one's window count with the
    /// new values from the next pass.
    pub fn apply_owner_settings(
        &self,
        settings: cockpit_config::SessionSettings,
    ) -> cockpit_config::SessionSettings {
        std::mem::replace(
            &mut *self
                .owner_settings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            settings,
        )
    }

    /// The state database the sessions live in, for the daemon's views that join them with
    /// other capabilities' tables (a session's task work, #157).
    pub fn database(&self) -> &Arc<Database> {
        &self.database
    }

    fn session_lock(&self, id: &SessionId) -> Arc<SessionLock> {
        let mut operations = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(operations.entry(id.clone()).or_default())
    }

    async fn lock(&self, id: &SessionId) -> Operation {
        let session = self.session_lock(id);
        Operation {
            _guard: Arc::clone(&session.operation).lock_owned().await,
            session,
        }
    }

    /// The session's lock for a stop, refused with [`Error::LaunchInProgress`] while a
    /// launch holds it or starts while the stop waits for it.
    async fn lock_to_stop(
        &self,
        id: &SessionId,
        operation: &'static str,
    ) -> Result<Operation, Error> {
        let session = self.session_lock(id);
        // One wait for the whole loop, so the stop keeps its place in the lock's queue when
        // a launch started and finished while it waited.
        let lock = Arc::clone(&session.operation).lock_owned();
        tokio::pin!(lock);
        let guard = loop {
            // Registered before the flag is read, so a launch starting in between wakes it.
            let launch_started = session.launch_started.notified();
            tokio::pin!(launch_started);
            launch_started.as_mut().enable();
            if session.launches.load(Ordering::SeqCst) > 0 {
                return Err(Error::LaunchInProgress {
                    session: id.clone(),
                    operation,
                });
            }
            tokio::select! {
                biased;
                guard = &mut lock => break guard,
                () = &mut launch_started => {}
            }
        };
        Ok(Operation {
            _guard: guard,
            session,
        })
    }

    /// Sessions not ended, or every session with `include_ended`, by creation.
    pub async fn list(&self, include_ended: bool) -> Result<Vec<Session>, Error> {
        let read = self.database.read().await?;
        store::sessions(&read, include_ended).await
    }

    pub async fn get(&self, id: &SessionId) -> Result<Session, Error> {
        let read = self.database.read().await?;
        store::session(&read, id).await
    }

    /// `cockpit session new`: records the session, then launches a new conversation in
    /// `request.dir`, or in a new parallel clone the session occupies.
    pub async fn start(&self, request: StartRequest) -> Result<Session, Error> {
        self.tmux()?;
        let provider = self.provider(request.provider.as_deref())?;
        self.check_launch_choices(
            Some(provider.name().as_str()),
            request.model.as_deref(),
            request.effort.as_deref(),
        )
        .await?;
        let dir = self
            .blocking("resolve the directory", move || canonical_dir(request.dir))
            .await?;
        let (checkout, relative) = self.checkout_containing(&dir).await?;
        let checkout = match &request.clone_branch {
            Some(branch) => {
                cockpit_projects::create_parallel_clone(
                    &self.database,
                    &self.git,
                    checkout.project.as_str(),
                    branch,
                )
                .await?
            }
            None => checkout,
        };
        let holds_checkout = checkout.kind != CheckoutKind::Main;
        // Collected from components: joining an empty `relative` would add a trailing `/`.
        let working_dir: PathBuf = checkout.path.join(relative).components().collect();
        let id = SessionId::generate()?;
        // Held through the first launch: a restart may take a `launching` session over, so
        // it must not find this one mid-launch.
        let operation = self.lock(&id).await;
        let write = self.database.write().await?;
        store::insert_session(
            &write,
            &NewSession {
                id: &id,
                provider: provider.name(),
                checkout: checkout.id,
                holds_checkout,
                working_dir: &working_dir,
                model: request.model.as_deref(),
                effort: request.effort.as_deref(),
                started_by: request.started_by.as_ref(),
            },
        )
        .await?;
        if holds_checkout {
            cockpit_projects::claim(&write, checkout.id, &holder(&id)).await?;
        }
        write.commit().await?;
        tracing::info!(session = %id, provider = %provider.name(), "starting a session");
        let launched = self
            .launch(
                &operation,
                &id,
                LaunchKind::Start,
                LaunchPurpose::Work,
                Conversation::New,
                request.first_message.as_deref(),
            )
            .await?;
        self.after_early_exit(launched).await
    }

    /// Refuses a model or effort `provider` would not launch, before a session is recorded or
    /// the owner is asked to start one (#701): the provider reports the capability and knows
    /// the value. [`Provider::launch`] only builds words, so a trial build is the provider's
    /// own check; the paths it is given are never used.
    pub async fn check_launch_choices(
        &self,
        provider: Option<&str>,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Result<(), Error> {
        if model.is_none() && effort.is_none() {
            return Ok(());
        }
        for (kind, value) in [("model", model), ("effort", effort)] {
            if value.is_some_and(|value| value.trim().is_empty()) {
                return Err(Error::InvalidValue(cockpit_provider_api::InvalidValue {
                    kind,
                    value: value.unwrap_or_default().to_owned(),
                    rule: "leave it out to use the harness's own setting",
                }));
            }
        }
        let provider = Arc::clone(self.provider(provider)?);
        let (model, effort) = (model.map(str::to_owned), effort.map(str::to_owned));
        self.blocking("check the model and effort", move || {
            provider.launch(&LaunchRequest {
                working_dir: Path::new("/"),
                session_dir: Path::new("/"),
                conversation: &Conversation::New,
                first_message: None,
                effort: effort.as_deref(),
                model: model.as_deref(),
                remote_control: false,
            })?;
            Ok(())
        })
        .await
    }

    /// Stops the agent and launches the same conversation again, in place. A session whose
    /// agent's status is unknown, because an error or a crash left it `launching` or
    /// `stopping`, restarts too (#157): the stop observes the agent's execution first,
    /// stops it if it still runs, then launches; it is never a no-op.
    pub async fn restart(&self, id: &SessionId) -> Result<Session, Error> {
        self.tmux()?;
        let operation = self.lock(id).await;
        let session = self.get(id).await?;
        self.restart_conversation(&session)?;
        self.restart_from(&operation, id, &RESTARTABLE, LaunchPurpose::Work)
            .await
    }

    /// Restarts a running session's agent on the current start files (`restart --all` and
    /// its deferred restart), keeping its finished label; the caller holds its lock.
    async fn restart_locked(&self, lock: &Operation, id: &SessionId) -> Result<Session, Error> {
        self.restart_from(lock, id, &[SessionState::Running], LaunchPurpose::Upkeep)
            .await
    }

    /// Restarts the running sessions whose start files changed since their launch, so
    /// they run on the current ones (#146, decided in #177). One whose agent waits at its
    /// prompt restarts now; a busy one is marked, and the periodic pass restarts it once its
    /// agent reports waiting, so no turn is cut. One failure does not stop the others.
    pub async fn restart_all(&self) -> Result<RestartAll, Error> {
        self.tmux()?;
        let running = {
            let read = self.database.read().await?;
            store::sessions_in(&read, &["running"]).await?
        };
        let mut outcome = RestartAll::default();
        for listed in running {
            match self.restart_if_changed(&listed.id).await {
                Ok(RestartStep::Current) => {}
                Ok(RestartStep::Restarted(restarted)) => outcome.restarted.push(restarted),
                Ok(RestartStep::Deferred(deferred, changed)) => {
                    outcome.deferred.push((deferred, changed));
                }
                Err(error) => outcome.failed.push((listed.id, error)),
            }
        }
        Ok(outcome)
    }

    /// `restart_all`'s step for one session, decided under its lock on a fresh read of the
    /// session and its agent's report, so an operation or report since the listing counts.
    async fn restart_if_changed(&self, id: &SessionId) -> Result<RestartStep, Error> {
        let operation = self.lock(id).await;
        let Some((session, agent_report)) = self.running_with_report(id).await? else {
            return Ok(RestartStep::Current);
        };
        let changed = self.changed_start_files(&session).await?;
        if changed.is_empty() {
            return Ok(RestartStep::Current);
        }
        self.restart_conversation(&session)?;
        if turn_refusal(&session, agent_report.as_ref()).is_none() {
            return Ok(RestartStep::Restarted(
                self.restart_locked(&operation, id).await?,
            ));
        }
        let write = self.database.write().await?;
        store::request_restart(&write, id).await?;
        let deferred = store::session(&write, id).await?;
        write.commit().await?;
        Ok(RestartStep::Deferred(deferred, changed))
    }

    /// The labels of the start files a running session's agent read at launch that changed
    /// since, as `cockpit-config` compares them. A session in any other state launches with
    /// the current files when it runs again, so it has none.
    pub async fn changed_start_files(&self, session: &Session) -> Result<Vec<String>, Error> {
        let Some(launch) = &session.launch else {
            return Ok(Vec::new());
        };
        if !matches!(
            session.state,
            SessionState::Running | SessionState::Launching
        ) {
            return Ok(Vec::new());
        }
        let snapshot = {
            let read = self.database.read().await?;
            store::start_files(&read, launch.number, session.provider.as_str()).await?
        };
        let start_files = self.settings.start_files.clone();
        self.blocking("compare the start files", move || {
            Ok(snapshot
                .changed(&start_files)
                .into_iter()
                .map(str::to_owned)
                .collect())
        })
        .await
    }

    /// Stops the agent and keeps the conversation to wake later. The owner may put a
    /// working agent to sleep; only losing the conversation is refused.
    pub async fn sleep(&self, id: &SessionId) -> Result<Session, Error> {
        self.tmux()?;
        let operation = self.lock_to_stop(id, "sleep").await?;
        let session = self.get(id).await?;
        if let Some(refusal) = self.sleep_refusal(&session) {
            return Err(Error::SleepRefused {
                session: id.clone(),
                refusal,
            });
        }
        self.stop(
            &operation,
            id,
            &[SessionState::Running],
            StopTarget::Asleep,
            "sleep",
            None,
        )
        .await
    }

    /// Launches an asleep session's conversation again: the one path every wake (owner,
    /// open, message, todo, phone) takes; `cause` is kept in its transition history. A
    /// session that is not asleep is refused.
    pub async fn wake(&self, id: &SessionId, cause: WakeCause) -> Result<Session, Error> {
        self.tmux()?;
        let operation = self.lock(id).await;
        self.wake_locked(&operation, id, cause).await
    }

    /// For callers that need the agent running, whatever its state now (opening its
    /// terminal, delivering a message or a todo): wakes an asleep session and answers a
    /// launching or running one as it is. Any other state is refused, as by [`Self::wake`].
    pub async fn ensure_awake(&self, id: &SessionId, cause: WakeCause) -> Result<Session, Error> {
        let operation = self.lock(id).await;
        let session = self.get(id).await?;
        if matches!(
            session.state,
            SessionState::Running | SessionState::Launching
        ) {
            return Ok(session);
        }
        self.tmux()?;
        self.wake_locked(&operation, id, cause).await
    }

    async fn wake_locked(
        &self,
        operation: &Operation,
        id: &SessionId,
        cause: WakeCause,
    ) -> Result<Session, Error> {
        let session = self.get(id).await?;
        // Checked first: a session that is not asleep has nothing to wake, whatever its
        // conversation (#647).
        if session.state != SessionState::Asleep {
            return Err(Error::WrongState {
                session: id.clone(),
                state: session.state,
                operation: "wake",
            });
        }
        let conversation = self.resumable(&session, "wake")?;
        // Waking reopens the conversation as resume does, so it is refused the same way.
        self.refuse_if_agent_alive_elsewhere(&session, "wake")
            .await?;
        let write = self.database.write().await?;
        refuse_if_active_elsewhere(&write, &session, "wake").await?;
        store::transition(
            &write,
            id,
            &[SessionState::Asleep],
            SessionState::Launching,
            Some(cause.reason()),
            "wake",
        )
        .await?;
        write.commit().await?;
        self.launch(
            operation,
            id,
            LaunchKind::Wake,
            LaunchPurpose::Work,
            conversation,
            None,
        )
        .await
    }

    /// Reopens an ended or failed session's conversation. A failed launch may have left its
    /// agent running, so the previous agent is stopped first. Refused while the conversation
    /// still runs in another session (#157).
    pub async fn resume(&self, id: &SessionId) -> Result<Session, Error> {
        self.tmux()?;
        let operation = self.lock(id).await;
        let session = self.get(id).await?;
        let conversation = self.resumable(&session, "resume")?;
        self.refuse_if_agent_alive_elsewhere(&session, "resume")
            .await?;
        if matches!(session.state, SessionState::Ended | SessionState::Failed) {
            self.stop_previous_agent(&session).await?;
        }
        let write = self.database.write().await?;
        refuse_if_active_elsewhere(&write, &session, "resume").await?;
        store::transition(
            &write,
            id,
            &[SessionState::Ended, SessionState::Failed],
            SessionState::Launching,
            None,
            "resume",
        )
        .await?;
        if session.holds_checkout {
            cockpit_projects::claim(&write, session.checkout, &holder(id)).await?;
        }
        write.commit().await?;
        let launched = self
            .launch(
                &operation,
                id,
                LaunchKind::Resume,
                LaunchPurpose::Work,
                conversation,
                None,
            )
            .await?;
        self.after_early_exit(launched).await
    }

    /// Refuses to launch `session`'s conversation while another session's agent of it is
    /// still alive, as `cockpit-processes` observes its execution now (report 04: an
    /// execution is identified by its supervisor identity, never a PID): a failed launch
    /// may have left one running whatever its session's state says.
    async fn refuse_if_agent_alive_elsewhere(
        &self,
        session: &Session,
        operation: &'static str,
    ) -> Result<(), Error> {
        let Some(conversation) = &session.conversation else {
            return Ok(());
        };
        let holders = {
            let read = self.database.read().await?;
            store::conversation_holders(&read, &session.provider, conversation, &session.id).await?
        };
        for holder in holders {
            let Some(launch) = &holder.launch else {
                continue;
            };
            let execution = self
                .processes
                .observe(&self.database, launch.execution)
                .await?;
            if execution.phase.is_occupying() {
                return Err(Error::CannotResume {
                    session: session.id.clone(),
                    operation,
                    reason: format!(
                        "its conversation is still alive as session {}'s agent (execution {})",
                        holder.id, launch.execution.0
                    ),
                });
            }
        }
        Ok(())
    }

    /// Ends the session for the owner: stops its agent, releases the clone it occupies. A
    /// launch or a stop left unfinished by an error is stopped as an end. A failed launch may
    /// have left its agent running (#221), so a failed session's latest agent is stopped
    /// first, as resume does; one not observed gone refuses the end and the session stays
    /// failed.
    pub async fn end(&self, id: &SessionId) -> Result<Session, Error> {
        self.end_by(id, &Ender::Owner).await
    }

    /// [`Self::end`] for `ender`, whom the session's reason then names (#713). Who may end
    /// which session is the caller's rule: this only records who did.
    pub async fn end_by(&self, id: &SessionId, ender: &Ender) -> Result<Session, Error> {
        self.end_by_when(id, ender, async |_| Ok::<(), Error>(()))
            .await
            .map(|(session, ())| session)
    }

    /// [`Self::end_by`] once `decide` allows it (#713). `decide` sees the session under its
    /// lock, right before its agent is stopped, so what it checks (a grant, the task list)
    /// is the state the end acts on; an error from it refuses the end, which does not begin.
    /// Its value comes back with the ended session, for the caller's record of the end.
    pub async fn end_by_when<T, E: From<Error>>(
        &self,
        id: &SessionId,
        ender: &Ender,
        decide: impl AsyncFnOnce(&Session) -> Result<T, E>,
    ) -> Result<(Session, T), E> {
        let reason = ender.reason();
        let operation = self.lock_to_stop(id, "end").await?;
        let session = self.get(id).await?;
        let decided = decide(&session).await?;
        let ended = self.end_locked(&operation, id, session, &reason).await?;
        Ok((ended, decided))
    }

    /// The end itself, under `operation`'s lock: see [`Self::end`].
    async fn end_locked(
        &self,
        operation: &Operation,
        id: &SessionId,
        session: Session,
        reason: &str,
    ) -> Result<Session, Error> {
        if matches!(
            session.state,
            SessionState::Running | SessionState::Launching | SessionState::Stopping { .. }
        ) {
            self.tmux()?;
            return self
                .stop(
                    operation,
                    id,
                    &[session.state],
                    StopTarget::Ended,
                    "end",
                    Some(reason),
                )
                .await;
        }
        // A session whose first launch failed before its agent was prepared has no launch
        // and no terminal: nothing to stop, so it ends without tmux.
        if session.state == SessionState::Failed && session.launch.is_some() {
            self.tmux()?;
            self.stop_previous_agent(&session).await?;
        }
        let write = self.database.write().await?;
        let ended = store::transition(
            &write,
            id,
            &[SessionState::Asleep, SessionState::Failed],
            SessionState::Ended,
            Some(reason),
            "end",
        )
        .await?;
        give_up_on_end(&write, &ended).await?;
        write.commit().await?;
        self.remove_session_folders(id).await?;
        Ok(ended)
    }

    /// Records a design link (any http or https URL) on the session.
    pub async fn link(&self, id: &SessionId, link: &str) -> Result<Session, Error> {
        validate_link(link)?;
        let _operation = self.lock(id).await;
        let write = self.database.write().await?;
        let session = store::set_design_link(&write, id, link).await?;
        write.commit().await?;
        Ok(session)
    }

    /// Reboot restore, run once at daemon start after the processes reconciled and before
    /// requests are accepted. A session that was active and whose agent is gone is reopened
    /// with its conversation, without a key press; a reopen that fails leaves the session
    /// `failed` with the reason and is not retried. Stops under way are finished. Only a
    /// state-database error stops the start: one finishing a stop (#251), or one that kept
    /// a failure from being recorded.
    pub async fn restore(&self) -> Result<Vec<Restored>, Error> {
        if let Err(error) = self.tmux() {
            // Their agents may still run in a server this daemon cannot reach; a daemon
            // started with tmux restores them.
            tracing::warn!(%error, "active sessions were left as they are, unrestored");
            return Ok(Vec::new());
        }
        let active = {
            let read = self.database.read().await?;
            store::sessions_in(&read, &["launching", "running", "stopping"]).await?
        };
        self.terminate_stopping_agents(&active).await;
        let mut restored = Vec::new();
        for session in &active {
            restored.push(self.restore_session(session).await?);
        }
        Ok(restored)
    }

    /// Terminates the running agents of the sessions left `stopping`, all at once: an agent
    /// that ignores SIGTERM waits out the stop's grace, and nothing is accepted until every
    /// stop is finished (#254). Only the waits overlap; the sessions are then restored one
    /// by one, so their terminals never race on the tmux server, and a stop this pass could
    /// not confirm is retried, and reported, there.
    async fn terminate_stopping_agents(&self, active: &[Session]) {
        let mut terminating = Vec::new();
        for session in active {
            let (SessionState::Stopping { .. }, Some(launch)) = (session.state, &session.launch)
            else {
                continue;
            };
            let (database, processes, stop) = (
                Arc::clone(&self.database),
                self.processes.clone(),
                self.settings.stop,
            );
            let execution = launch.execution;
            terminating.push(tokio::spawn(async move {
                let phase = cockpit_processes::execution(&database, execution)
                    .await?
                    .phase;
                // A start never confirmed is released in order, with its terminal (#256).
                if phase == Phase::Running {
                    processes.terminate(&database, execution, stop).await?;
                }
                Ok::<(), cockpit_processes::Error>(())
            }));
        }
        let mut panic = None;
        for task in terminating {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, "a stalled stop is retried with its session");
                }
                Err(error) if error.is_panic() => panic = Some(error.into_panic()),
                Err(error) => tracing::warn!(%error, "a stop's termination was cancelled"),
            }
        }
        // Raised once every task is over, so none keeps stopping agents behind it.
        if let Some(panic) = panic {
            std::panic::resume_unwind(panic);
        }
    }

    async fn restore_session(&self, session: &Session) -> Result<Restored, Error> {
        let id = session.id.clone();
        // Uncontended: restore runs before the daemon accepts requests. A relaunch needs it.
        let operation = self.lock(&id).await;
        let SessionState::Stopping { to } = session.state else {
            return match self.restore_one(&operation, session).await {
                Ok(outcome) => Ok(outcome),
                Err(error) => {
                    self.fail_restore(session, format!("restore failed: {error}"))
                        .await
                }
            };
        };
        // Marked through its stop phase, as `stop` marks a restart.
        let _launching = (to == StopTarget::Restart).then(|| operation.launching());
        // The stopping state does not keep who asked for an end; the restore names the owner,
        // whose daemon it is, as before #713.
        let stopped = match self
            .finish_stop(&id, to, Some(&Ender::Owner.reason()))
            .await
        {
            Ok(stopped) => stopped,
            Err(error) if error.is_database() => return Err(error),
            Err(error) => return Ok(Restored::StopNotFinished(id, error.to_string())),
        };
        if to != StopTarget::Restart {
            return Ok(Restored::StopFinished(id, stopped.state));
        }
        // Who asked for the interrupted restart is not kept: a working one ([`LaunchPurpose`]).
        let conversation = self.restart_conversation(&stopped);
        match self
            .relaunch(&operation, &stopped, LaunchPurpose::Work, conversation)
            .await
        {
            Ok(relaunched) => Ok(Restored::StopFinished(id, relaunched.state)),
            Err(error) => self.reopen_failed(&id, error).await,
        }
    }

    /// A reopen on restore that failed. The launch leaves the session `failed`, which is
    /// reported and never retried; one still `launching` could not be recorded failed, so
    /// the error stops the daemon's start, as a failure `fail_restore` cannot record does.
    async fn reopen_failed(&self, id: &SessionId, error: Error) -> Result<Restored, Error> {
        if self.get(id).await?.state != SessionState::Failed {
            return Err(error);
        }
        Ok(Restored::Failed(
            id.clone(),
            format!("restore failed: {error}"),
        ))
    }

    async fn restore_one(&self, lock: &Operation, session: &Session) -> Result<Restored, Error> {
        let id = &session.id;
        if self.agent_is_running(session).await? {
            // A daemon older than a self permission launched it without that grant (#284);
            // granted with the move to running, so neither commits without the other.
            let write = self.database.write().await?;
            if session.state != SessionState::Running {
                store::transition(
                    &write,
                    id,
                    &[session.state],
                    SessionState::Running,
                    None,
                    "restore",
                )
                .await?;
            }
            grant_self_permissions(&write, &authorization_session(id)?).await?;
            write.commit().await?;
            return Ok(Restored::StillRunning(id.clone()));
        }
        if let Err(error) = self.stop_previous_agent(session).await {
            return self
                .fail_restore(session, format!("restore refused: {error}"))
                .await;
        }
        let kind = if self.launching_for_restart(session).await? {
            LaunchKind::Restart
        } else {
            LaunchKind::Restore
        };
        // A session left `launching` was in a launch a crash interrupted: a working one
        // ([`LaunchPurpose`]).
        let purpose = if session.state == SessionState::Launching {
            LaunchPurpose::Work
        } else {
            self.transition(
                id,
                &[session.state],
                SessionState::Launching,
                None,
                "restore",
            )
            .await?;
            LaunchPurpose::Upkeep
        };
        let conversation = match kind {
            LaunchKind::Restart => self.restart_conversation(session),
            _ => self.resumable(session, "restore"),
        };
        let conversation = match conversation {
            Ok(conversation) => conversation,
            Err(error) => {
                return self
                    .fail_restore(session, format!("restore failed: {error}"))
                    .await;
            }
        };
        match self
            .launch(lock, id, kind, purpose, conversation, None)
            .await
        {
            Ok(_) => Ok(Restored::Reopened(id.clone())),
            Err(error) => self.reopen_failed(id, error).await,
        }
    }

    /// Whether a `launching` session is a restart whose stop committed before a crash
    /// stopped the daemon: its latest transition is that stop's move to launching (#252).
    async fn launching_for_restart(&self, session: &Session) -> Result<bool, Error> {
        if session.state != SessionState::Launching {
            return Ok(false);
        }
        let read = self.database.read().await?;
        store::latest_transition_is(
            &read,
            &session.id,
            SessionState::Stopping {
                to: StopTarget::Restart,
            },
            SessionState::Launching,
        )
        .await
    }

    /// Leaves a session restore could not reopen `failed` with `reason`, from whatever
    /// active state it was in.
    async fn fail_restore(&self, session: &Session, reason: String) -> Result<Restored, Error> {
        let current = self.get(&session.id).await?;
        self.move_to_failed(&session.id, &[current.state], &reason, "restore")
            .await?;
        tracing::warn!(session = %session.id, reason, "a session could not be restored");
        Ok(Restored::Failed(session.id.clone(), reason))
    }

    /// One pass of the lifecycle's own work, run periodically by the daemon: running
    /// sessions whose agent exited end, idle ones sleep, and asleep ones past the expiry
    /// end. A failure is reported per session and looked at again on the next pass.
    pub async fn tick(&self) -> Result<TickReport, Error> {
        let mut report = TickReport::default();
        let (running, asleep, ended) = {
            let read = self.database.read().await?;
            (
                // Without tmux a running session's terminal can be neither checked nor
                // closed; it waits, unrestored, for a daemon started with tmux.
                if self.tmux.is_ok() {
                    store::sessions_in(&read, &["running"]).await?
                } else {
                    Vec::new()
                },
                store::sessions_in(&read, &["asleep"]).await?,
                store::sessions_in(&read, &["ended"]).await?,
            )
        };
        for session in ended {
            let _operation = self.lock(&session.id).await;
            if self.get(&session.id).await?.state == SessionState::Ended
                && let Err(error) = self.remove_session_folders(&session.id).await
            {
                report.errors.push((session.id, error.to_string()));
            }
        }
        // Running sessions that will not go to sleep by themselves, with their launch:
        // looked at for a missing agent once every session's lock is released.
        let mut never_sleeping = Vec::new();
        for listed in running {
            let id = listed.id.clone();
            let operation = self.lock(&id).await;
            // Re-read under the lock: an owner operation may have replaced the launch since,
            // and the agent may have reported.
            let (session, agent_report) = match self.running_with_report(&id).await {
                Ok(Some(read)) => read,
                Ok(None) => continue,
                Err(error) => {
                    report.errors.push((id, error.to_string()));
                    continue;
                }
            };
            match self.fail_if_terminal_lost(&session).await {
                Ok(Some(_)) => {
                    report.terminal_lost.push(id);
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    report.errors.push((id, error.to_string()));
                    continue;
                }
            }
            match self.end_if_exited(&session).await {
                Ok(Some(over)) => report.record_exit(&over),
                Ok(None) => {
                    let agent_report = agent_report.as_ref();
                    if session.restart_pending && turn_refusal(&session, agent_report).is_none() {
                        match self.restart_locked(&operation, &id).await {
                            Ok(_) => report.restarted.push(id),
                            Err(error) => report.errors.push((id, error.to_string())),
                        }
                        continue;
                    }
                    match self.flag_if_unreported(&session, agent_report).await {
                        Ok(true) => {
                            report.unreported.push(id);
                            continue;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            report.errors.push((id, error.to_string()));
                            continue;
                        }
                    }
                    if let Some(refusal) = self.idle_sleep_refusal(&session, agent_report) {
                        // The idle sleep stops everything a session runs, so one only
                        // waiting for its idle window needs nothing more. Any other may
                        // be left with no agent and never end by itself.
                        if !matches!(refusal, SleepRefusal::Idle { .. })
                            && let Some(launch) = &session.launch
                        {
                            never_sleeping.push((id, launch.number));
                        }
                        continue;
                    }
                    match self
                        .stop(
                            &operation,
                            &id,
                            &[SessionState::Running],
                            StopTarget::Asleep,
                            "sleep",
                            None,
                        )
                        .await
                    {
                        Ok(_) => report.slept.push(id),
                        Err(error) => report.errors.push((id, error.to_string())),
                    }
                }
                Err(error) => report.errors.push((id, error.to_string())),
            }
        }
        let expiry =
            i64::try_from(self.owner_settings().asleep_expiry.as_secs()).unwrap_or(i64::MAX);
        for session in asleep {
            let _operation = self.lock(&session.id).await;
            match self.expire_if_due(&session.id, expiry).await {
                Ok(false) => {}
                Ok(true) => report.expired.push(session.id),
                Err(error) => report.errors.push((session.id, error.to_string())),
            }
        }
        self.flag_agentless(never_sleeping, &mut report).await;
        self.drop_idle_session_locks();
        Ok(report)
    }

    /// Drops the session locks nothing holds (#406). Both counts are read under the map's
    /// lock, which every new holder takes first: an operation or a counted step holds the
    /// entry, and a guard taken with `try_lock_owned` holds only its mutex.
    fn drop_idle_session_locks(&self) {
        let mut operations = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        operations.retain(|_, session| session_lock_is_held(session));
    }

    /// Reports the `never_sleeping` launches whose agent is gone (#406). No session lock is
    /// held: looking at terminals takes tmux calls, long enough that an owner's send or
    /// cancel would find its session busy, and the report is written only for a session
    /// still running that launch, read in the transaction that writes it.
    async fn flag_agentless(&self, never_sleeping: Vec<(SessionId, i64)>, report: &mut TickReport) {
        let gone = self.launches_with_agent_gone(never_sleeping).await;
        // The window counts passes that saw the agent gone without a break: a session this
        // pass did not see so starts again.
        self.agentless
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|id, seen| gone.get(id) == Some(&seen.launch));
        for (id, launch) in gone {
            match self.flag_if_agentless(&id, launch).await {
                Ok(true) => report.agentless.push(id),
                Ok(false) => {}
                Err(error) => report.errors.push((id, error.to_string())),
            }
        }
    }

    /// The `sessions` whose terminal is open with the agent's pane dead, by launch: the
    /// process tmux started there exited, which is certain, while the pass saw the
    /// execution occupied. A terminal that is closed says nothing of its agent and is left
    /// out, and so is one that cannot be read; a look that fails altogether is logged and
    /// answers none.
    async fn launches_with_agent_gone(
        &self,
        sessions: Vec<(SessionId, i64)>,
    ) -> HashMap<SessionId, i64> {
        let terminals: Vec<(SessionId, i64, SessionName)> = sessions
            .into_iter()
            .filter_map(|(id, launch)| {
                let name = terminal_name(&id).ok()?;
                Some((id, launch, name))
            })
            .collect();
        let Ok(tmux) = self.tmux() else {
            return HashMap::new();
        };
        if terminals.is_empty() {
            return HashMap::new();
        }
        let looked = self
            .blocking("look at the agents' terminals", move || {
                let open = tmux.sessions()?;
                let mut gone = HashMap::new();
                for (id, launch, name) in terminals {
                    if !open.contains(&name) {
                        continue;
                    }
                    match agent_pane_info(&tmux, &name) {
                        Ok(pane) if matches!(pane.state, PaneState::Exited { .. }) => {
                            gone.insert(id, launch);
                        }
                        Ok(_) => {}
                        // Closed since the list, or unreadable: the next pass looks again.
                        Err(error) => {
                            tracing::debug!(session = %id, %error, "a terminal could not be read");
                        }
                    }
                }
                Ok(gone)
            })
            .await;
        looked.unwrap_or_else(|error| {
            tracing::warn!(%error, "the agents' terminals could not be looked at");
            HashMap::new()
        })
    }

    /// Shows a running session whose agent is gone, while processes it started still run,
    /// as waiting on the owner with the command that ends it (#406), once every pass has
    /// seen it so for [`Settings::agentless_after`]; `true` when it did. Told once: a later
    /// pass writes nothing more. Nothing is stopped: what still runs may be work the owner
    /// wants. An agent's leftovers that keep its terminal open are not seen, since tmux then
    /// shows the pane running.
    async fn flag_if_agentless(&self, id: &SessionId, launch: i64) -> Result<bool, Error> {
        let since = {
            let mut agentless = self
                .agentless
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let seen = agentless.entry(id.clone()).or_insert(Agentless {
                launch,
                since: store::now(),
                is_reported: false,
            });
            if seen.is_reported {
                return Ok(false);
            }
            seen.since
        };
        let window = i64::try_from(self.settings.agentless_after.as_secs()).unwrap_or(i64::MAX);
        if store::now().saturating_sub(since) < window {
            return Ok(false);
        }
        let reason = format!(
            "its agent exited, but processes it started still run; \
             `torlek session end {id}` stops them"
        );
        let write = self.database.write().await?;
        let is_running = store::session(&write, id).await?.state == SessionState::Running;
        let is_flagged = is_running
            && crate::reports::flag_agentless_launch(
                &write,
                &authorization_session(id)?,
                launch,
                &reason,
            )
            .await?;
        if is_flagged {
            write.commit().await?;
        } else {
            write.rollback().await?;
        }
        if let Some(seen) = self
            .agentless
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(id)
        {
            seen.is_reported = true;
        }
        Ok(is_flagged)
    }

    /// Shows a running session whose agent has not reported for its launch within the
    /// deadline as waiting on the owner, with why (#702); `true` when it did. Only for a
    /// provider whose bridge reports at launch: for any other, silence is the agent waiting
    /// for its first prompt.
    async fn flag_if_unreported(
        &self,
        session: &Session,
        agent_report: Option<&SessionReport>,
    ) -> Result<bool, Error> {
        let Some(launch) = &session.launch else {
            return Ok(false);
        };
        // A report moved from the agent channel names no launch; it is the running agent's.
        let has_reported = agent_report.is_some_and(|agent_report| {
            agent_report.launch.is_none() || agent_report.launch == Some(launch.number)
        });
        let deadline = i64::try_from(self.settings.report_deadline.as_secs()).unwrap_or(i64::MAX);
        // Whole seconds: strictly past, so a launch late in its second waits the full deadline.
        let is_overdue = store::now().saturating_sub(launch.launched_at) > deadline;
        if has_reported || !is_overdue {
            return Ok(false);
        }
        // A provider no longer configured has no bridge that could report.
        let reports_at_launch = self
            .provider(Some(session.provider.as_str()))
            .is_ok_and(|provider| provider.reports_at_launch());
        if !reports_at_launch {
            return Ok(false);
        }
        let reason = format!(
            "{} has not reported {deadline} s after its launch: its terminal may wait on a \
             screen of its own (folder trust, sign-in, update); attach to answer it",
            session.provider
        );
        let write = self.database.write().await?;
        let is_flagged = crate::reports::flag_unreported_launch(
            &write,
            &authorization_session(&session.id)?,
            launch.number,
            &reason,
        )
        .await?;
        if is_flagged {
            write.commit().await?;
        } else {
            write.rollback().await?;
        }
        Ok(is_flagged)
    }

    /// The session, when it runs, with its agent's latest report.
    async fn running_with_report(
        &self,
        id: &SessionId,
    ) -> Result<Option<(Session, Option<SessionReport>)>, Error> {
        let read = self.database.read().await?;
        let session = store::session(&read, id).await?;
        if session.state != SessionState::Running {
            return Ok(None);
        }
        let agent_report =
            crate::reports::latest_report(&read, &authorization_session(id)?).await?;
        Ok(Some((session, agent_report)))
    }

    /// Ends an asleep session once it slept `expiry` seconds, counted from its last move to
    /// asleep: other changes, such as a design link, do not restart the count.
    async fn expire_if_due(&self, id: &SessionId, expiry: i64) -> Result<bool, Error> {
        // Read first: most passes find nothing due, and need no write lock. The caller holds
        // the session's lock, so the state read stays current.
        let asleep_since = {
            let read = self.database.read().await?;
            store::entered_asleep_at(&read, id).await?
        };
        let Some(asleep_since) = asleep_since else {
            return Ok(false);
        };
        if store::now().saturating_sub(asleep_since) < expiry {
            return Ok(false);
        }
        let write = self.database.write().await?;
        let ended = store::transition(
            &write,
            id,
            &[SessionState::Asleep],
            SessionState::Ended,
            Some("asleep past the expiry; resume reopens it"),
            "expire",
        )
        .await?;
        give_up_on_end(&write, &ended).await?;
        write.commit().await?;
        self.remove_session_folders(id).await?;
        Ok(true)
    }

    /// Only the exit part of [`Self::tick`], run more often by the daemon (#632): a running
    /// session whose agent is gone ends, or fails, within seconds rather than at the next
    /// tick. The same failure handling as the tick. A live agent is looked at without the
    /// session's lock: the owner's send and cancel refuse a held lock as busy, and the sweep
    /// runs every second.
    pub async fn sweep_exits(&self) -> Result<TickReport, Error> {
        let mut report = TickReport::default();
        if self.tmux.is_err() {
            return Ok(report);
        }
        let running = {
            let read = self.database.read().await?;
            store::sessions_in(&read, &["running"]).await?
        };
        for listed in running {
            let id = listed.id.clone();
            let Some(launch) = &listed.launch else {
                continue;
            };
            match self
                .processes
                .observe(&self.database, launch.execution)
                .await
            {
                Ok(execution) if execution.phase.is_occupying() => continue,
                Ok(_) => {}
                // A host query that failed is asked again a second later; the tick reports
                // a lasting one, without a warning per session every second.
                Err(
                    cockpit_processes::Error::HostCommand { .. }
                    | cockpit_processes::Error::Io { .. },
                ) => continue,
                Err(error) => {
                    report.errors.push((id, error.to_string()));
                    continue;
                }
            }
            let _operation = self.lock(&id).await;
            let session = match self.get(&id).await {
                Ok(session) if session.state == SessionState::Running => session,
                Ok(_) => continue,
                Err(error) => {
                    report.errors.push((id, error.to_string()));
                    continue;
                }
            };
            match self.fail_if_terminal_lost(&session).await {
                Ok(Some(_)) => {
                    report.terminal_lost.push(id);
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    report.errors.push((id, error.to_string()));
                    continue;
                }
            }
            match self.end_if_exited(&session).await {
                Ok(Some(over)) => report.record_exit(&over),
                Ok(None) => {}
                Err(error) => report.errors.push((id, error.to_string())),
            }
        }
        Ok(report)
    }

    /// Answers a launch the way its agent is after the early-exit window: one that exited
    /// within it leaves the session `failed` or `ended`, with its exit (#632). A check that
    /// fails is logged: the launch itself succeeded, and the sweep looks again.
    async fn after_early_exit(&self, launched: Session) -> Result<Session, Error> {
        let deadline = tokio::time::Instant::now() + self.settings.early_exit_window;
        loop {
            match self.end_if_exited(&launched).await {
                Ok(Some(over)) => return Ok(over),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(session = %launched.id, %error, "a new agent's exit could not be checked");
                    return Ok(launched);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(launched);
            }
            tokio::time::sleep(EARLY_EXIT_POLL).await;
        }
    }

    /// Ends a running session whose agent was observed gone, or leaves it `failed` when the
    /// agent exited with a failure status (#632); the session, when it did.
    async fn end_if_exited(&self, session: &Session) -> Result<Option<Session>, Error> {
        let Some(launch) = &session.launch else {
            return Ok(None);
        };
        let execution = self
            .processes
            .observe(&self.database, launch.execution)
            .await?;
        if execution.phase.is_occupying() {
            return Ok(None);
        }
        let Some(exit) = self.close_terminal_reading_exit(&session.id).await? else {
            // The terminal went since the pass looked: as when it was already gone.
            let failed = self
                .settle_over(
                    session,
                    launch,
                    &crate::exits::unknown(),
                    &terminal_lost_reason(&session.id),
                    true,
                )
                .await?;
            return Ok(Some(failed));
        };
        let reason = crate::exits::reason(exit.status);
        let over = self
            .settle_over(session, launch, &exit, &reason, exit.status.is_failure())
            .await?;
        tracing::info!(
            session = %session.id,
            state = %over.state,
            status = %exit.status,
            output = ?exit.output,
            "a session's agent exited"
        );
        Ok(Some(over))
    }

    /// Fails a running session whose terminal, its tmux session, is gone, and the agent
    /// with it if it outlived the terminal (#844); the session, when it did. Nobody can
    /// attach a terminal that is gone, so it is no longer shown as running: `resume`
    /// reopens its conversation. A look at tmux that fails leaves the session as it is.
    /// The caller holds the session's lock, so no launch replaces the terminal meanwhile.
    async fn fail_if_terminal_lost(&self, session: &Session) -> Result<Option<Session>, Error> {
        let Some(launch) = &session.launch else {
            return Ok(None);
        };
        let tmux = self.tmux()?;
        let name = terminal_name(&session.id)?;
        let is_open = self
            .blocking("look at the session's terminal", move || {
                Ok(tmux.sessions()?.contains(&name))
            })
            .await?;
        if is_open {
            return Ok(None);
        }
        self.stop_previous_agent(session).await?;
        let failed = self
            .settle_over(
                session,
                launch,
                &crate::exits::unknown(),
                &terminal_lost_reason(&session.id),
                true,
            )
            .await?;
        Ok(Some(failed))
    }

    /// Moves a running session whose agent is gone to `failed` or `ended` with `reason`,
    /// recording how the agent exited and releasing what an over session holds.
    async fn settle_over(
        &self,
        session: &Session,
        launch: &Launch,
        exit: &AgentExit,
        reason: &str,
        is_failure: bool,
    ) -> Result<Session, Error> {
        let write = self.database.write().await?;
        let over = if is_failure {
            let failed = store::transition(
                &write,
                &session.id,
                &[SessionState::Running],
                SessionState::Failed,
                Some(reason),
                "fail",
            )
            .await?;
            // Its agent is gone as with an ended session: it holds nothing until resumed.
            give_up_on_end(&write, &failed).await?;
            failed
        } else {
            let ended = store::transition(
                &write,
                &session.id,
                &[SessionState::Running],
                SessionState::Ended,
                Some(reason),
                "end",
            )
            .await?;
            give_up_on_end(&write, &ended).await?;
            ended
        };
        store::insert_exit(&write, launch, exit).await?;
        // Read again: the snapshot the transition returned predates the exit.
        let over = store::session(&write, &over.id).await?;
        write.commit().await?;
        self.remove_session_folders(&session.id).await?;
        Ok(over)
    }

    /// Closes the session's terminal like [`Self::close_terminal`], first reading how its
    /// agent exited, in the same step; `None` when the terminal was already gone. A read
    /// that fails is logged and reads as an unknown exit: the session still ends.
    async fn close_terminal_reading_exit(
        &self,
        id: &SessionId,
    ) -> Result<Option<AgentExit>, Error> {
        let tmux = self.tmux()?;
        let name = terminal_name(id)?;
        let counted = TerminalStep::start(&self.session_lock(id));
        let session = id.clone();
        self.blocking("read the agent's exit and close the terminal", move || {
            let _counted = counted;
            let exit = crate::exits::read(&tmux, &name).unwrap_or_else(|error| {
                tracing::warn!(%session, %error, "an exited agent's terminal could not be read");
                Some(crate::exits::unknown())
            });
            close_if_open(&tmux, &name)?;
            Ok(exit)
        })
        .await
    }

    /// Why the owner may not put `session` to sleep: it is not running, or could not be
    /// woken.
    fn sleep_refusal(&self, session: &Session) -> Option<SleepRefusal> {
        if session.state != SessionState::Running {
            return Some(SleepRefusal::NotRunning(session.state));
        }
        if session.conversation.is_none() {
            // What it needs, said for the case a live session is in (#647).
            return Some(SleepRefusal::CannotResume(
                "its provider has not named the conversation yet; it does once its agent \
                 reports, usually after a first turn: send it a prompt, then retry"
                    .to_owned(),
            ));
        }
        if let Err(error) = self.resumable(session, "wake") {
            return Some(SleepRefusal::CannotResume(match error {
                Error::CannotResume { reason, .. } => reason,
                other => other.to_string(),
            }));
        }
        None
    }

    /// Why the periodic pass may not put `session` to sleep: on top of the owner's
    /// refusals, its agent's latest report must say it has waited at its prompt for the
    /// whole idle window.
    fn idle_sleep_refusal(
        &self,
        session: &Session,
        agent_report: Option<&SessionReport>,
    ) -> Option<SleepRefusal> {
        if let Some(refusal) = self.sleep_refusal(session) {
            return Some(refusal);
        }
        if let Some(refusal) = turn_refusal(session, agent_report) {
            return Some(refusal);
        }
        let agent_report = agent_report?;
        let window =
            i64::try_from(self.owner_settings().idle_sleep_after.as_secs()).unwrap_or(i64::MAX);
        let remaining = agent_report.reported_at.saturating_add(window) - store::now();
        if remaining > 0 {
            return Some(SleepRefusal::Idle { remaining });
        }
        None
    }

    /// The conversation to launch again for `operation`, or why there is none.
    fn resumable(&self, session: &Session, operation: &'static str) -> Result<Conversation, Error> {
        let cannot = |reason: &str| Error::CannotResume {
            session: session.id.clone(),
            operation,
            reason: reason.to_owned(),
        };
        let provider = self.provider(Some(session.provider.as_str()))?;
        if !provider.capabilities().supports(Capability::Resume) {
            return Err(cannot("its agent's provider cannot resume a conversation"));
        }
        let conversation = session.conversation.clone().ok_or_else(|| {
            cannot("its provider has not named the conversation; start a new session")
        })?;
        Ok(Conversation::Resume(conversation))
    }

    /// The conversation a restart launches: the session's own, or a new one while its
    /// provider has named none (#647). A provider names its conversation at launch or with
    /// its agent's first report, so an unnamed one has had no turn to lose. Decided on the
    /// session alone, the same before the stop, after it and on restore.
    fn restart_conversation(&self, session: &Session) -> Result<Conversation, Error> {
        if session.conversation.is_none() {
            self.provider(Some(session.provider.as_str()))?;
            return Ok(Conversation::New);
        }
        self.resumable(session, "restart")
    }

    /// [`store::transition`] in a transaction of its own.
    async fn transition(
        &self,
        id: &SessionId,
        from: &[SessionState],
        to: SessionState,
        reason: Option<&str>,
        operation: &'static str,
    ) -> Result<Session, Error> {
        debug_assert!(
            !matches!(to, SessionState::Ended | SessionState::Failed),
            "moves to ended or failed go through give_up_on_end or move_to_failed"
        );
        let write = self.database.write().await?;
        let changed = store::transition(&write, id, from, to, reason, operation).await?;
        write.commit().await?;
        Ok(changed)
    }

    /// Moves a session whose launch failed to `failed`, keeping the reason.
    async fn fail(&self, id: &SessionId, reason: &str) -> Result<Session, Error> {
        self.move_to_failed(id, &[SessionState::Launching], reason, "fail")
            .await
    }

    /// Moves the session from one of `from` to `failed` and revokes its self grants: a failed
    /// launch may have left an agent running, and it must not report or ask for the session.
    async fn move_to_failed(
        &self,
        id: &SessionId,
        from: &[SessionState],
        reason: &str,
        operation: &'static str,
    ) -> Result<Session, Error> {
        let write = self.database.write().await?;
        let failed = store::transition(
            &write,
            id,
            from,
            SessionState::Failed,
            Some(reason),
            operation,
        )
        .await?;
        revoke_self_grants(&write, id).await?;
        store::clear_restart_request(&write, id).await?;
        write.commit().await?;
        Ok(failed)
    }

    /// Moves the session from one of `from` to stopping, then stops its agent; a restart
    /// goes through [`Self::restart_from`].
    /// `ended_reason` is what an end leaves as the session's reason; a sleep has none.
    async fn stop(
        &self,
        _lock: &Operation,
        id: &SessionId,
        from: &[SessionState],
        to: StopTarget,
        operation: &'static str,
        ended_reason: Option<&str>,
    ) -> Result<Session, Error> {
        assert!(
            to != StopTarget::Restart,
            "a restart goes through restart_from, which relaunches it"
        );
        self.transition(id, from, SessionState::Stopping { to }, None, operation)
            .await?;
        self.finish_stop(id, to, ended_reason).await
    }

    /// Moves the session from one of `from` to stopping, stops its agent and launches its
    /// conversation again for `purpose`. Marked as a launch from here, its stop phase
    /// included.
    async fn restart_from(
        &self,
        lock: &Operation,
        id: &SessionId,
        from: &[SessionState],
        purpose: LaunchPurpose,
    ) -> Result<Session, Error> {
        let to = StopTarget::Restart;
        self.transition(id, from, SessionState::Stopping { to }, None, "restart")
            .await?;
        let _launching = lock.launching();
        let stopped = self.finish_stop(id, to, None).await?;
        // Chosen again after the stop: the provider may have named it in between.
        let conversation = self.restart_conversation(&stopped);
        self.relaunch(lock, &stopped, purpose, conversation).await
    }

    /// Stops a stopping session's agent and moves it on: asleep, ended (with `ended_reason`
    /// as its reason), or `launching` for a restart, which [`Self::relaunch`] then launches.
    /// An agent not observed gone puts the session back to running.
    async fn finish_stop(
        &self,
        id: &SessionId,
        to: StopTarget,
        ended_reason: Option<&str>,
    ) -> Result<Session, Error> {
        let session = self.get(id).await?;
        let stopping = SessionState::Stopping { to };
        if let Err(error) = self.stop_previous_agent(&session).await {
            if let Error::StopUnconfirmed { .. } = &error {
                self.transition(
                    id,
                    &[stopping],
                    SessionState::Running,
                    Some("its agent termination could not be confirmed"),
                    "stop",
                )
                .await?;
            }
            return Err(error);
        }
        let (state, reason) = match to {
            StopTarget::Asleep => (SessionState::Asleep, None),
            StopTarget::Ended => (SessionState::Ended, ended_reason),
            StopTarget::Restart => (SessionState::Launching, None),
        };
        let write = self.database.write().await?;
        let stopped = store::transition(&write, id, &[stopping], state, reason, "stop").await?;
        if to == StopTarget::Ended {
            give_up_on_end(&write, &stopped).await?;
        }
        write.commit().await?;
        if to == StopTarget::Ended {
            self.remove_session_folders(id).await?;
        }
        Ok(stopped)
    }

    /// Launches `conversation` for a restart once its stop finished. A failure, choosing
    /// the conversation included, leaves the session `failed` with the reason, and is
    /// returned.
    async fn relaunch(
        &self,
        lock: &Operation,
        stopped: &Session,
        purpose: LaunchPurpose,
        conversation: Result<Conversation, Error>,
    ) -> Result<Session, Error> {
        let conversation = match conversation {
            Ok(conversation) => conversation,
            Err(error) => {
                self.fail(&stopped.id, &error.to_string()).await?;
                return Err(error);
            }
        };
        self.launch(
            lock,
            &stopped.id,
            LaunchKind::Restart,
            purpose,
            conversation,
            None,
        )
        .await
    }

    /// Stops the agent of the session's latest launch, if any, and closes its terminal;
    /// refused while that agent may still run.
    async fn stop_previous_agent(&self, session: &Session) -> Result<(), Error> {
        if let Some(launch) = &session.launch {
            self.release_if_never_started(&session.id, launch.execution)
                .await?;
            let termination = self
                .processes
                .terminate(&self.database, launch.execution, self.settings.stop)
                .await?;
            if let Termination::Unconfirmed { diagnostics } = termination {
                tracing::warn!(
                    session = %session.id,
                    execution = launch.execution.0,
                    ?diagnostics,
                    "a session's agent termination is unconfirmed"
                );
                return Err(Error::StopUnconfirmed {
                    session: session.id.clone(),
                    execution: launch.execution,
                });
            }
        }
        self.close_terminal(&session.id).await
    }

    /// Releases the agent's execution if its start was never confirmed and its command never
    /// ran: a crash after the launch was recorded, before its terminal ran it, leaves one
    /// (#256), and a stop would wait out the launch timeout and then refuse. The terminal is
    /// closed first, so it cannot start the command after the check; a command that started
    /// is left to the stop.
    async fn release_if_never_started(
        &self,
        id: &SessionId,
        execution: ExecutionId,
    ) -> Result<(), Error> {
        let phase = cockpit_processes::execution(&self.database, execution)
            .await?
            .phase;
        if phase != Phase::Launching {
            return Ok(());
        }
        self.close_terminal(id).await?;
        self.processes
            .abandon_hosted(&self.database, execution)
            .await?;
        Ok(())
    }

    /// Launches `conversation` for a session already `launching`. A failure leaves the
    /// session `failed` with the reason, and is returned.
    async fn launch(
        &self,
        lock: &Operation,
        id: &SessionId,
        kind: LaunchKind,
        purpose: LaunchPurpose,
        conversation: Conversation,
        first_message: Option<&str>,
    ) -> Result<Session, Error> {
        let _launching = lock.launching();
        match self
            .try_launch(id, kind, purpose, &conversation, first_message)
            .await
        {
            Ok(session) => Ok(session),
            Err(error) => {
                tracing::warn!(session = %id, ?kind, %error, "a session launch failed");
                self.fail(id, &error.to_string()).await?;
                Err(error)
            }
        }
    }

    async fn try_launch(
        &self,
        id: &SessionId,
        kind: LaunchKind,
        purpose: LaunchPurpose,
        conversation: &Conversation,
        first_message: Option<&str>,
    ) -> Result<Session, Error> {
        let tmux = self.tmux()?;
        self.wait_for_terminal_steps(id).await?;
        let session = self.get(id).await?;
        let session_dir = self.session_dir(id);
        let temp_dir = self.session_temp_dir(id);
        {
            let session_dir = session_dir.clone();
            let temp_dir = temp_dir.clone();
            let counted = TerminalStep::start(&self.session_lock(id));
            self.blocking("create the session's directories", move || {
                let _counted = counted;
                create_session_dir(&session_dir)?;
                create_session_dir(&temp_dir)
            })
            .await?;
        }
        let command = {
            let provider = Arc::clone(self.provider(Some(session.provider.as_str()))?);
            let working_dir = session.working_dir.clone();
            let session_dir = session_dir.clone();
            let conversation = conversation.clone();
            let first_message = first_message.map(str::to_owned);
            let (model, effort) = (session.model.clone(), session.effort.clone());
            // Off the async workers and bounded: the contract says it does not block, but
            // a provider that does would otherwise hold the launch forever (#272).
            let counted = TerminalStep::start(&self.session_lock(id));
            self.blocking("build the provider's command", move || {
                let _counted = counted;
                Ok(provider.launch(&LaunchRequest {
                    working_dir: &working_dir,
                    session_dir: &session_dir,
                    conversation: &conversation,
                    first_message: first_message.as_deref(),
                    effort: effort.as_deref(),
                    model: model.as_deref(),
                    remote_control: false,
                })?)
            })
            .await?
        };
        if let Conversation::Resume(requested) = conversation
            && command.conversation.as_ref() != Some(requested)
        {
            return Err(Error::Provider(ProviderError {
                reason: format!("it did not resume conversation {requested}"),
            }));
        }
        let channel_session = authorization_session(id)?;
        // Taken before the agent runs: a file changed between the two reads as changed.
        let start_files = {
            let declared = self.settings.start_files.clone();
            let provider = session.provider.as_str().to_owned();
            self.blocking("capture the start files", move || {
                Ok(StartFileSnapshot::capture(&declared, &provider))
            })
            .await?
        };
        let delivery_launch = command.clone();
        let mut environment = self.launch_environment();
        environment.extend(command.environment);
        let mut environment = with_temp_dir(environment, &temp_dir);
        environment.push(("TORLEK_SESSION".into(), id.as_str().into()));
        // Every launch goes through the sandbox (#61); one it refuses, such as any under
        // `agents.sandbox = "required"` while no backend exists (#828), fails the launch
        // before anything is reserved. The agent gets its whole checkout,
        // not just the folder it starts in: Git and the project's own files live at its root.
        let (checkout, _) = self.checkout_containing(&session.working_dir).await?;
        let output_project = checkout.project.as_str().to_owned();
        let setting = self.owner_settings().sandbox;
        let github = self.github_access(setting, &output_project).await?;
        // Kept for the clone's harness trust, recorded once the launch passed its gates.
        let clone_trust = matches!(checkout.kind, CheckoutKind::Parallel { .. }).then(|| {
            (
                checkout.clone(),
                with_github_token(environment.clone(), None),
            )
        });
        let tools = self
            .sandbox_tools(setting, &checkout.path, &session.working_dir, &environment)
            .await?;
        let needs = provider_needs(
            self.provider(Some(session.provider.as_str()))?.as_ref(),
            &tools.ecosystems,
        )?;
        let decisions = self
            .launch_decisions(
                setting,
                &output_project,
                &cockpit_sandbox::Subject::Session {
                    session: id.as_str().to_owned(),
                },
            )
            .await?;
        let prepared = self.prepare_sandbox(
            setting,
            id.as_str(),
            session.provider.as_str(),
            checkout.path,
            session.working_dir.clone(),
            session_dir,
            temp_dir,
            environment,
            github,
            needs,
            decisions,
            tools,
            command.program,
            command.arguments,
        )?;
        // Removed if this launch returns before its agent is released.
        let mut staged_credentials = StagedCredentials(
            prepared
                .staged
                .as_ref()
                .and_then(cockpit_sandbox::Staged::credentials_path),
        );
        if let Some(staged) = prepared.staged.clone() {
            let counted = TerminalStep::start(&self.session_lock(id));
            self.blocking("write the sandbox's launch spec", move || {
                let _counted = counted;
                staged.write().map_err(|source| Error::Io {
                    action: "write",
                    path: staged.spec_path(),
                    source,
                })
            })
            .await?;
        }
        if let cockpit_sandbox::Enforcement::Unsandboxed { reason } = &prepared.evidence.enforcement
        {
            tracing::warn!(
                session = %id,
                %reason,
                "an agent launches UNSANDBOXED: {}",
                cockpit_config::UNSANDBOXED_RISK
            );
        }
        // The execution is reserved in the transaction that records the launch, so a crash
        // never leaves one the session does not name (#256); the sandbox evidence is recorded
        // with it, so a launch the database names always says how its agent ran.
        let (token, execution) = {
            let write = self.database.write().await?;
            cockpit_projects::require_execution_start_allowed(&write).await?;
            cockpit_projects::require_checkout_available(&write, session.checkout).await?;
            let token = cockpit_agent_channel::mint(&write, &channel_session).await?;
            grant_self_permissions(&write, &channel_session).await?;
            let execution = self
                .processes
                .reserve(&write, self.database.epoch())
                .await?;
            cockpit_processes::attribute_output(
                &write,
                execution,
                self.database.epoch(),
                Some(&output_project),
                Some(id.as_str()),
                cockpit_processes::OutputMode::Hosted,
                false,
            )
            .await?;
            store::insert_launch(&write, id, kind, purpose, execution, &start_files).await?;
            cockpit_sandbox::record(&write, execution.0, id.as_str(), &prepared.evidence).await?;
            crate::delivery_context::record_launch(
                &write,
                execution.0,
                &prepared.reach,
                &delivery_launch,
            )
            .await?;
            write.commit().await?;
            (token, execution)
        };
        if let Some((checkout, agent_environment)) = &clone_trust {
            self.trust_clone(
                id,
                session.provider.as_str(),
                checkout,
                &session.working_dir,
                agent_environment,
            )
            .await;
        }
        let mut environment = prepared.environment;
        environment.push((LAUNCH_TOKEN_VARIABLE.into(), token.as_str().into()));
        let spec = LaunchSpec {
            program: prepared.program,
            arguments: prepared.arguments,
            working_dir: session.working_dir.clone(),
            environment,
            terminal: None,
        };
        let hosted = self
            .processes
            .prepare_reserved_hosted(&self.database, execution, &spec, &TERMINAL_VARIABLES)
            .await?;
        let terminal = SessionLaunch {
            name: terminal_name(id)?,
            working_dir: session.working_dir.clone(),
            command: hosted.command,
            environment: Vec::new(),
            columns: self.settings.columns,
            rows: self.settings.rows,
        };
        let opened = self.open_terminal(id, tmux, terminal).await;
        if let Err(error) = opened {
            // A timed-out open may still create the terminal and start the agent later; one
            // refused for lack of a permit never ran.
            if matches!(error, Error::StepTimedOut { .. }) {
                self.discard_launch(id, execution).await;
            } else {
                self.processes
                    .abandon_hosted(&self.database, execution)
                    .await?;
            }
            return Err(error);
        }
        let confirmed = match self.confirm_agent(&channel_session, execution).await {
            Ok(execution) => execution,
            Err(error) => {
                self.discard_launch(id, execution).await;
                return Err(error);
            }
        };
        staged_credentials.hand_over();
        let write = self.database.write().await?;
        if let Some(named) = &command.conversation {
            store::set_conversation(&write, id, named).await?;
        }
        store::clear_restart_request(&write, id).await?;
        let running = store::transition(
            &write,
            id,
            &[SessionState::Launching],
            SessionState::Running,
            None,
            "launch",
        )
        .await?;
        write.commit().await?;
        tracing::info!(session = %id, ?kind, execution = confirmed.id.0, "a session runs");
        Ok(running)
    }

    /// Asks the provider to record its harness's trust of a parallel clone before the agent
    /// runs there, so the agent does not stop on the harness's folder-trust dialog (#702,
    /// `docs/decisions/2026-11-04-702-claude-folder-trust.md`): only for a clone of a project
    /// in the owner's `projects.trusted` whose origin is its project's. Never fails the
    /// launch: a refusal leaves the dialog to the owner, and the periodic pass shows the
    /// session as waiting on them.
    async fn trust_clone(
        &self,
        id: &SessionId,
        provider: &str,
        checkout: &Checkout,
        working_dir: &Path,
        environment: &[(OsString, OsString)],
    ) {
        let provider = match self.provider(Some(provider)) {
            Ok(provider) => Arc::clone(provider),
            Err(error) => {
                tracing::warn!(session = %id, %error, "a clone's harness trust is not recorded");
                return;
            }
        };
        let owner_environment = self.settings.owner_environment.clone();
        let git = self.git.clone();
        let checkout = checkout.clone();
        let working_dir = working_dir.to_path_buf();
        let environment = environment.to_vec();
        let counted = TerminalStep::start(&self.session_lock(id));
        let recorded = self
            .blocking("record the harness's trust of the clone", move || {
                let _counted = counted;
                Ok(record_clone_trust(
                    provider.as_ref(),
                    &owner_environment,
                    &git,
                    &checkout,
                    &working_dir,
                    &environment,
                ))
            })
            .await
            .unwrap_or_else(|error| Err(error.to_string()));
        match recorded {
            Ok(WorkspaceTrust::Recorded) => {
                tracing::info!(session = %id, "recorded the harness's trust of a parallel clone");
            }
            Ok(WorkspaceTrust::AlreadyTrusted | WorkspaceTrust::NotKept) => {}
            Err(reason) => {
                tracing::warn!(session = %id, %reason, "a clone's harness trust is not recorded");
            }
        }
    }

    /// The variables every agent starts with before its provider's own: the base ones, then
    /// the owner's desktop session while the owner shares it.
    fn launch_environment(&self) -> Vec<(OsString, OsString)> {
        let mut environment = self.settings.base_environment.clone();
        if self.desktop_session() == crate::DesktopSession::Shared {
            environment.extend(
                self.desktop_environment
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .cloned(),
            );
        }
        environment
    }

    /// What a launch in `project` under `setting` gets of GitHub (#754, #832), read now so a
    /// token the owner replaced reaches the next launch.
    async fn github_access(
        &self,
        setting: cockpit_config::AgentSandbox,
        project: &str,
    ) -> Result<GithubAccess, Error> {
        let environment = self.settings.owner_environment.clone();
        let project = project.to_owned();
        self.blocking("read the project's GitHub token", move || {
            github_access(&environment, setting, &project)
        })
        .await
    }

    /// How `program` runs for a launch of `provider` in `checkout_root`, started in
    /// `start_dir`, with the session's own `session_dir` and the project's `github` access,
    /// under the owner's `setting`, read once for the launch: unchanged under
    /// `agents.sandbox = "off"`; under `"required"`, the native sandbox's launcher in its
    /// place, or refused on a host without it (#829).
    #[allow(
        clippy::too_many_arguments,
        reason = "one launch's facts, each passed straight into the sandbox's request"
    )]
    fn prepare_sandbox(
        &self,
        setting: cockpit_config::AgentSandbox,
        session: &str,
        provider: &str,
        checkout_root: PathBuf,
        start_dir: PathBuf,
        session_dir: PathBuf,
        temp_dir: PathBuf,
        environment: Vec<(OsString, OsString)>,
        github: GithubAccess,
        needs: cockpit_sandbox::ProviderNeeds,
        decisions: cockpit_sandbox::EgressDecisions,
        tools: cockpit_sandbox::ToolNeeds,
        program: PathBuf,
        arguments: Vec<OsString>,
    ) -> Result<cockpit_sandbox::Prepared, Error> {
        // A sandboxed agent never holds the project's token, which can push (#857): pushes go
        // through `torlek push`. The session proxy adds the project's API token instead, on
        // the GitHub hosts the launch reaches: by its declared needs, or by the owner's
        // grants and allows at launch (#832, #674).
        let (token, api_credential) = match setting {
            cockpit_config::AgentSandbox::Required => (
                None,
                github.api_token.as_ref().map(|api_token| {
                    let reach = cockpit_sandbox::Allowlist {
                        program: String::new(),
                        entries: needs.egress.clone(),
                    }
                    .with_decisions(&decisions);
                    github_api_credential(api_token, &reach)
                }),
            ),
            cockpit_config::AgentSandbox::Off => (github.token.as_ref(), None),
        };
        let mut secrets = vec![cockpit_sandbox::Secret {
            // The agent presents it on the local agent socket, never in an HTTP header a
            // proxy could swap a placeholder in.
            variable: LAUNCH_TOKEN_VARIABLE.to_owned(),
            handling: cockpit_sandbox::SecretHandling::PassThrough,
        }];
        if token.is_some() {
            secrets.push(cockpit_sandbox::Secret {
                // Git sends it inside Basic credentials, which a header-swapping proxy does
                // not decode, so a placeholder would never be swapped back: `git push` would
                // fail.
                variable: cockpit_config::GITHUB_TOKEN_VARIABLE.to_owned(),
                handling: cockpit_sandbox::SecretHandling::PassThrough,
            });
        }
        self.settings
            .sandbox
            .prepare(
                setting,
                &cockpit_sandbox::LaunchRequest {
                    session: session.to_owned(),
                    provider: provider.to_owned(),
                    working_dir: checkout_root,
                    start_dir,
                    session_dir,
                    temp_dir,
                    environment: with_github_token(environment, token),
                    secrets,
                    denied: github.hidden_owner_login,
                    desktop: self.desktop_session(),
                    needs,
                    tools,
                    credentials: api_credential
                        .as_ref()
                        .and_then(|bound| bound.credential.clone())
                        .into_iter()
                        .collect(),
                    decisions,
                },
                program,
                arguments,
            )
            .map_err(Error::Sandbox)
            .map(|mut prepared| {
                if let Some(bound) = &api_credential {
                    prepared.evidence.rules.extend(bound.unbound_rules());
                }
                prepared
            })
    }

    /// The owner's decisions (#674) for `subject` of `project` when it launches: the project's
    /// grants and the subject's earlier answers, which hold from the agent's first connection,
    /// not from the daemon's next write, and which a bound credential counts as reach (#832).
    /// None under `"off"`, whose launch has no proxy to apply them.
    async fn launch_decisions(
        &self,
        setting: cockpit_config::AgentSandbox,
        project: &str,
        subject: &cockpit_sandbox::Subject,
    ) -> Result<cockpit_sandbox::EgressDecisions, Error> {
        if setting == cockpit_config::AgentSandbox::Off {
            return Ok(cockpit_sandbox::EgressDecisions::default());
        }
        let read = self.database.read().await?;
        cockpit_sandbox::egress_decisions(&read, project, subject)
            .await
            .map_err(|error| match error {
                cockpit_sandbox::NetworkError::Sql(source) => Error::Sql(source),
                other => Error::InvalidStoredRow {
                    reason: other.to_string(),
                },
            })
    }

    /// [`tool_needs`] for a launch in `checkout`, started in `start_dir`, with `environment`
    /// under `setting`, off the async workers. None under `"off"`, whose agent keeps the
    /// owner's own caches and configuration, nor without a backend, whose launch is refused.
    async fn sandbox_tools(
        &self,
        setting: cockpit_config::AgentSandbox,
        checkout: &Path,
        start_dir: &Path,
        environment: &[(OsString, OsString)],
    ) -> Result<cockpit_sandbox::ToolNeeds, Error> {
        let has_backend = self.settings.sandbox.backend().missing_reason().is_none();
        if setting == cockpit_config::AgentSandbox::Off || !has_backend {
            return Ok(cockpit_sandbox::ToolNeeds::none());
        }
        let checkout = checkout.to_path_buf();
        let start_dir = start_dir.to_path_buf();
        let environment = environment.to_vec();
        self.blocking("read the checkout's toolchains", move || {
            tool_needs(&checkout, &start_dir, &environment)
        })
        .await
    }

    /// The temp folder the dry run stands in for a session's with.
    fn dry_run_temp_dir(&self) -> PathBuf {
        self.session_temps.join(DRY_RUN_SESSION)
    }

    /// `cockpit sandbox show`: the sandbox a new session of `provider` in `dir` would get,
    /// without launching anything. The agent's own program and arguments are the provider's
    /// at launch; the dry run shows `<agent>` in their place.
    pub async fn sandbox_dry_run(
        &self,
        dir: PathBuf,
        provider: Option<&str>,
    ) -> Result<cockpit_sandbox::Prepared, Error> {
        let provider = self.provider(provider)?.name().as_str().to_owned();
        let dir = self
            .blocking("resolve the directory", move || canonical_dir(dir))
            .await?;
        let (checkout, _) = self.checkout_containing(&dir).await?;
        let setting = self.owner_settings().sandbox;
        let github = self
            .github_access(setting, checkout.project.as_str())
            .await?;
        let environment = with_temp_dir(self.launch_environment(), &self.dry_run_temp_dir());
        let tools = self
            .sandbox_tools(setting, &checkout.path, &checkout.path, &environment)
            .await?;
        let needs = provider_needs(self.provider(Some(&provider))?.as_ref(), &tools.ecosystems)?;
        let decisions = self
            .launch_decisions(
                setting,
                checkout.project.as_str(),
                &cockpit_sandbox::Subject::Session {
                    session: DRY_RUN_SESSION.to_owned(),
                },
            )
            .await?;
        self.prepare_sandbox(
            setting,
            DRY_RUN_SESSION,
            &provider,
            checkout.path.clone(),
            checkout.path,
            self.session_dirs.join(DRY_RUN_SESSION),
            self.dry_run_temp_dir(),
            environment,
            github,
            needs,
            decisions,
            tools,
            PathBuf::from("<agent>"),
            Vec::new(),
        )
    }

    /// Record the owner's measurements for the exact latest sandbox launch, serialized
    /// with restarts. A claim never changes a profile, capability or session grant.
    pub async fn record_sandbox_observations(
        &self,
        id: &SessionId,
        client_identity: &str,
        observations: &[String],
    ) -> Result<cockpit_sandbox::Evidence, Error> {
        let Ok(_guard) = Arc::clone(&self.session_lock(id).operation).try_lock_owned() else {
            return Err(Error::SessionBusy {
                session: id.clone(),
            });
        };
        let write = self.database.write().await?;
        store::session(&write, id).await?;
        cockpit_sandbox::record_observations(&write, id.as_str(), client_identity, observations)
            .await?;
        let evidence = cockpit_sandbox::latest_for_session(&write, id.as_str())
            .await?
            .ok_or_else(|| Error::InvalidStoredRow {
                reason: "recorded sandbox observation has no launch".into(),
            })?;
        write.commit().await?;
        Ok(evidence)
    }

    /// How the session's latest launch ran (#61), or `None` before its first launch.
    pub async fn sandbox_evidence(
        &self,
        id: &SessionId,
    ) -> Result<Option<cockpit_sandbox::Evidence>, Error> {
        let read = self.database.read().await?;
        store::session(&read, id).await?;
        let Some(mut evidence) = cockpit_sandbox::latest_for_session(&read, id.as_str()).await?
        else {
            return Ok(None);
        };
        drop(read);
        if matches!(
            evidence.enforcement,
            cockpit_sandbox::Enforcement::Sandboxed { .. }
        ) {
            let sandbox = self.settings.sandbox.clone();
            let session = id.as_str().to_owned();
            let report = self
                .blocking("read the session proxy's counters", move || {
                    Ok(sandbox.egress_report(&session))
                })
                .await?;
            // Counters are observations: one that cannot be read is said, never fatal.
            match report {
                Ok(Some(report)) => evidence.observed.extend(report.describe()),
                Ok(None) => {}
                Err(reason) => evidence
                    .observed
                    .push(format!("egress counters unreadable: {reason}")),
            }
        }
        Ok(Some(evidence))
    }

    async fn confirm_agent(
        &self,
        session: &cockpit_authorization::SessionId,
        execution: ExecutionId,
    ) -> Result<cockpit_processes::Execution, Error> {
        let execution = self
            .processes
            .confirm_hosted(&self.database, execution)
            .await?;
        self.record_agent_root(session, execution.id, execution.process_group)
            .await?;
        // Only now may the agent run: its first request finds its root recorded (#316).
        self.processes
            .release_hosted(&self.database, execution.id)
            .await?;
        Ok(execution)
    }

    /// Undoes a launch whose terminal opened, or whose open timed out and may still open
    /// it, but whose agent could not be confirmed: closing the terminal hangs up the pane,
    /// then the execution is released or stopped.
    /// Best effort, since the launch already failed; what remains is logged.
    async fn discard_launch(&self, id: &SessionId, execution: ExecutionId) {
        if let Err(error) = self.close_terminal(id).await {
            tracing::warn!(session = %id, %error, "a failed launch's terminal could not be closed");
        }
        let stopped = async {
            self.processes
                .abandon_hosted(&self.database, execution)
                .await?;
            self.processes
                .terminate(&self.database, execution, self.settings.stop)
                .await
        };
        match stopped.await {
            Ok(Termination::Confirmed { .. }) => {}
            Ok(Termination::Unconfirmed { diagnostics }) => tracing::warn!(
                session = %id,
                execution = execution.0,
                ?diagnostics,
                "a failed launch's agent may still run; resume stops it first"
            ),
            Err(error) => tracing::warn!(
                session = %id,
                execution = execution.0,
                %error,
                "a failed launch's agent could not be stopped; resume stops it first"
            ),
        }
    }

    /// Records the agent's process as its launch's root, so its token binds from that tree
    /// only (report 17). Where process trees cannot be read (macOS, report 11 `validate`),
    /// the agent channel refuses the agent's requests; the session still runs.
    async fn record_agent_root(
        &self,
        session: &cockpit_authorization::SessionId,
        execution: ExecutionId,
        process_group: Option<i32>,
    ) -> Result<(), Error> {
        let Some(pid) = process_group.and_then(|group| u32::try_from(group).ok()) else {
            return Err(Error::InvalidStoredRow {
                reason: format!(
                    "session {}: a running agent has no process",
                    session.as_str()
                ),
            });
        };
        let identity = self
            .blocking("identify the agent's process", move || {
                Ok(cockpit_agent_channel::identity_of(pid))
            })
            .await?;
        match identity {
            Ok(root) => {
                let write = self.database.write().await?;
                cockpit_agent_channel::record_root(&write, session, &root).await?;
                crate::delivery_context::record_root(&write, execution.0, &root).await?;
                write.commit().await?;
            }
            Err(error) => tracing::warn!(
                session = session.as_str(),
                %error,
                "the agent's process cannot be identified; the agent channel will refuse it"
            ),
        }
        Ok(())
    }

    /// Whether the session's latest agent still runs: its execution was reattached and its
    /// terminal exists.
    async fn agent_is_running(&self, session: &Session) -> Result<bool, Error> {
        let Some(launch) = &session.launch else {
            return Ok(false);
        };
        let execution = cockpit_processes::execution(&self.database, launch.execution).await?;
        if execution.phase != Phase::Running {
            return Ok(false);
        }
        let tmux = self.tmux()?;
        let name = terminal_name(&session.id)?;
        self.blocking("list the terminals", move || {
            Ok(tmux.sessions()?.contains(&name))
        })
        .await
    }

    /// Closes the session's terminal. Never waits behind an earlier terminal step, so the
    /// owner can end a session whose launch hung in tmux (#272); a stale step that later
    /// closes or opens it acts on no newer launch, since none opens before it returns.
    async fn close_terminal(&self, id: &SessionId) -> Result<(), Error> {
        let tmux = self.tmux()?;
        let name = terminal_name(id)?;
        let counted = TerminalStep::start(&self.session_lock(id));
        self.blocking("close the terminal", move || {
            let _counted = counted;
            close_if_open(&tmux, &name)
        })
        .await
    }

    /// Opens a launch's terminal; [`Self::wait_for_terminal_steps`] ran first.
    async fn open_terminal(
        &self,
        id: &SessionId,
        tmux: Arc<Server>,
        terminal: SessionLaunch,
    ) -> Result<(), Error> {
        let counted = TerminalStep::start(&self.session_lock(id));
        self.blocking("open the terminal", move || {
            let _counted = counted;
            close_if_open(&tmux, &terminal.name)?;
            tmux.new_session(&terminal).map(drop).map_err(Error::from)
        })
        .await
    }

    /// Waits, at most the step timeout, until every earlier step that may close or open the
    /// session's terminal returned (#324), before a launch prepares anything. The caller
    /// holds the session's lock, so no step of it starts until the open: the ones counted
    /// are stale and only return.
    async fn wait_for_terminal_steps(&self, id: &SessionId) -> Result<(), Error> {
        let mut steps = self.session_lock(id).terminal_steps.subscribe();
        match tokio::time::timeout(
            self.settings.step_timeout,
            steps.wait_for(|steps| *steps == 0),
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(_) => Err(Error::TerminalBusy {
                session: id.clone(),
            }),
        }
    }

    // The caller holds the operation lock; counted work also excludes a later launch
    // if removal times out and continues on its blocking thread. Only an end removes them:
    // an asleep or failed session's next launch finds its temp files again.
    async fn remove_session_folders(&self, id: &SessionId) -> Result<(), Error> {
        if *self.session_lock(id).terminal_steps.borrow() != 0 {
            tracing::debug!(session = %id, "directory cleanup deferred until outstanding steps return");
            return Ok(());
        }
        let mut paths = vec![self.session_dir(id), self.session_temp_dir(id)];
        // A sandboxed session's own home, with its provider's conversations, outlives its
        // relaunches but not its end (#858).
        if let Some(home) = self.settings.sandbox.session_home(id.as_str()) {
            paths.push(home.map_err(Error::Sandbox)?);
        }
        // Its folder of mount sources holds the owner's Git identity (#859) and the folders
        // its sandbox mounted (#482).
        if let Some(mount_sources) = self.settings.sandbox.session_mount_sources(id.as_str()) {
            paths.push(mount_sources.map_err(Error::Sandbox)?);
        }
        let counted = TerminalStep::start(&self.session_lock(id));
        self.blocking("remove the session's directories", move || {
            let _counted = counted;
            // Each removal is tried, so a stuck runtime directory never keeps a large temp
            // folder on the disk; the first failure is returned.
            let mut failure = None;
            for path in paths {
                match remove_tree(&path) {
                    Ok(()) => {}
                    Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        failure.get_or_insert(Error::Io {
                            action: "remove",
                            path,
                            source,
                        });
                    }
                }
            }
            failure.map_or(Ok(()), Err)
        })
        .await
    }

    /// The session's own directory ([`LaunchRequest::session_dir`]); its launch creates it.
    pub fn session_dir(&self, id: &SessionId) -> PathBuf {
        self.session_dirs.join(id.as_str())
    }

    /// The session's own temporary folder, its agent's `TMPDIR`; its launch creates it and
    /// its end removes it.
    pub fn session_temp_dir(&self, id: &SessionId) -> PathBuf {
        self.session_temps.join(id.as_str())
    }

    /// The folder holding every session's temporary folder, which the storage report measures.
    pub fn session_temps(&self) -> &Path {
        &self.session_temps
    }

    /// Runs a blocking step off the async workers, holding one of the capability's permits
    /// (#325). Waits at most the step timeout for the permit, then at most the step timeout
    /// for the step (#272). A step that runs out of it keeps running, and keeps its permit
    /// until it returns, since started blocking work cannot be aborted (report 03): its
    /// caller undoes what it may still do.
    async fn blocking<T: Send + 'static>(
        &self,
        name: &'static str,
        step: impl FnOnce() -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let timeout = self.settings.step_timeout;
        let permits = Arc::clone(&self.blocking_permits);
        let Ok(permit) = tokio::time::timeout(timeout, permits.acquire_owned()).await else {
            return Err(Error::StepsBusy {
                step: name,
                limit: self.settings.blocking_steps,
            });
        };
        let permit = permit.expect("the blocking step permits are never closed");
        let running = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            step()
        });
        match tokio::time::timeout(timeout, running).await {
            Ok(joined) => joined.map_err(|error| Error::Blocking(error.to_string()))?,
            Err(_) => Err(Error::StepTimedOut {
                step: name,
                after: timeout,
            }),
        }
    }

    fn tmux(&self) -> Result<Arc<Server>, Error> {
        self.tmux
            .clone()
            .map_err(|reason| Error::TmuxUnavailable { reason })
    }

    fn provider(&self, name: Option<&str>) -> Result<&Arc<dyn Provider>, Error> {
        let available = || {
            self.providers
                .iter()
                .map(|provider| provider.name().to_string())
                .collect()
        };
        match name {
            Some(name) => self
                .providers
                .iter()
                .find(|provider| provider.name().as_str() == name)
                .ok_or_else(|| Error::UnknownProvider {
                    name: name.to_owned(),
                    available: available(),
                }),
            None => match self.providers.as_slice() {
                [only] => Ok(only),
                _ => Err(Error::ProviderNeeded {
                    available: available(),
                }),
            },
        }
    }

    /// The checkout whose working copy holds `dir` (the deepest, since clones live beside
    /// their project), and `dir` relative to it.
    async fn checkout_containing(&self, dir: &Path) -> Result<(Checkout, PathBuf), Error> {
        let read = self.database.read().await?;
        let checkouts = cockpit_projects::checkouts(&read, None).await?;
        checkouts
            .into_iter()
            .filter_map(|checkout| {
                let relative = dir.strip_prefix(&checkout.path).ok()?.to_path_buf();
                Some((checkout, relative))
            })
            .max_by_key(|(checkout, _)| checkout.path.components().count())
            .ok_or_else(|| Error::NotInProject {
                dir: dir.to_path_buf(),
            })
    }
}

/// What `restart_all` did with one session.
enum RestartStep {
    /// Not running, or its start files are current.
    Current,
    Restarted(Session),
    /// Busy; restarts once its agent waits. With the start files that changed.
    Deferred(Session, Vec<String>),
}

/// Why the session's agent is not between turns, waiting at its prompt with nothing asked:
/// its latest report, sent by the agent of the current launch, must say so. A report from
/// an earlier launch says nothing about this agent.
fn turn_refusal(session: &Session, agent_report: Option<&SessionReport>) -> Option<SleepRefusal> {
    let current_launch = session.launch.as_ref().map(|launch| launch.number);
    let Some(agent_report) = agent_report
        .filter(|agent_report| current_launch.is_some() && agent_report.launch == current_launch)
    else {
        return Some(SleepRefusal::NotReady);
    };
    if agent_report.needs_input.is_some() {
        return Some(SleepRefusal::NeedsInput);
    }
    match agent_report.state {
        Some(AgentState::Working) => Some(SleepRefusal::Working),
        Some(AgentState::Waiting) => None,
        None => Some(SleepRefusal::NotReady),
    }
}

/// Refuses to launch `session`'s conversation while another session holding it is active,
/// read in the transaction that moves `session` to launching: two resumes of sessions
/// sharing a conversation cannot both pass, since the first one's move is committed before
/// the second one reads.
async fn refuse_if_active_elsewhere(
    write: &cockpit_turso::WriteTransaction<'_>,
    session: &Session,
    operation: &'static str,
) -> Result<(), Error> {
    let Some(conversation) = &session.conversation else {
        return Ok(());
    };
    let holders =
        store::conversation_holders(write, &session.provider, conversation, &session.id).await?;
    let active = holders.into_iter().find(|holder| {
        matches!(
            holder.state,
            SessionState::Launching | SessionState::Running | SessionState::Stopping { .. }
        )
    });
    match active {
        Some(holder) => Err(Error::CannotResume {
            session: session.id.clone(),
            operation,
            reason: format!(
                "its conversation still runs in session {} ({})",
                holder.id, holder.state
            ),
        }),
        None => Ok(()),
    }
}

/// Ends the session's occupancy of its clone, if it holds one and still occupies it.
async fn release(
    write: &cockpit_turso::WriteTransaction<'_>,
    session: &Session,
) -> Result<(), Error> {
    if !session.holds_checkout {
        return Ok(());
    }
    match cockpit_projects::release(write, session.checkout, &holder(&session.id)).await {
        Ok(()) | Err(cockpit_projects::Error::NotOccupant { .. }) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// What an ended session gives up, in the transaction that ends it: its clone, its self
/// grants and the coordinator grants the owner gave it (#410). Every move to `ended` calls
/// this, and the move to `failed` of a session whose agent exited with a failure (#632).
async fn give_up_on_end(
    write: &cockpit_turso::WriteTransaction<'_>,
    ended: &Session,
) -> Result<(), Error> {
    release(write, ended).await?;
    store::clear_restart_request(write, &ended.id).await?;
    revoke_self_grants(write, &ended.id).await?;
    revoke_coordinator_grants(write, &ended.id).await
}

/// Revokes every coordinator grant the session holds: an ended session coordinates nothing,
/// and its card no longer shows them.
async fn revoke_coordinator_grants(
    write: &cockpit_turso::WriteTransaction<'_>,
    id: &SessionId,
) -> Result<(), Error> {
    let session = authorization_session(id)?;
    let coordinator_grants = cockpit_authorization::active_grants(write, &session)
        .await?
        .into_iter()
        .filter(|grant| grant.permission.is_coordinator());
    for grant in coordinator_grants {
        cockpit_authorization::revoke(write, grant.id).await?;
    }
    Ok(())
}

/// Grants the session the [`SELF_PERMISSIONS`] over itself; a grant it holds is kept as it
/// is, without advancing the authorization revision.
async fn grant_self_permissions(
    write: &cockpit_turso::WriteTransaction<'_>,
    session: &cockpit_authorization::SessionId,
) -> Result<(), Error> {
    let itself = Scope::Session(session.clone());
    for permission in SELF_PERMISSIONS {
        cockpit_authorization::grant(write, session, permission, &itself).await?;
    }
    Ok(())
}

/// Revokes the [`SELF_PERMISSIONS`] over itself a launch granted the session, so its agent
/// may no longer report or ask the owner. An owner grant of the same permission and scope is
/// the same grant and goes with it. Each revocation advances the authorization revision.
async fn revoke_self_grants(
    write: &cockpit_turso::WriteTransaction<'_>,
    id: &SessionId,
) -> Result<(), Error> {
    let session = authorization_session(id)?;
    let itself = Scope::Session(session.clone());
    let self_grants = cockpit_authorization::active_grants(write, &session)
        .await?
        .into_iter()
        .filter(|grant| SELF_PERMISSIONS.contains(&grant.permission) && grant.scope == itself);
    for grant in self_grants {
        cockpit_authorization::revoke(write, grant.id).await?;
    }
    Ok(())
}

/// The session as the authorization and agent-channel crates name it.
fn authorization_session(id: &SessionId) -> Result<cockpit_authorization::SessionId, Error> {
    cockpit_authorization::SessionId::new(id.as_str()).map_err(|error| Error::InvalidStoredRow {
        reason: error.to_string(),
    })
}

fn holder(id: &SessionId) -> Holder {
    Holder::new(id.as_str()).expect("a session id is a valid holder")
}

/// The reason of a session failed for its terminal being gone (#844).
fn terminal_lost_reason(id: &SessionId) -> String {
    format!(
        "its terminal is gone: its tmux session no longer exists; \
         `torlek session resume {id}` reopens it"
    )
}

fn terminal_name(id: &SessionId) -> Result<SessionName, Error> {
    Ok(SessionName::new(id.as_str())?)
}

/// Closes a session's terminal; a pane that exited stays until closed (`remain-on-exit`).
fn close_if_open(tmux: &Server, name: &SessionName) -> Result<(), Error> {
    if tmux.sessions()?.contains(name) {
        tmux.close_session(name)?;
    }
    Ok(())
}

fn canonical_dir(dir: PathBuf) -> Result<PathBuf, Error> {
    let canonical = dir.canonicalize().map_err(|source| Error::Io {
        action: "resolve",
        path: dir.clone(),
        source,
    })?;
    if !canonical.is_dir() {
        return Err(Error::NotInProject { dir: canonical });
    }
    Ok(canonical)
}

/// The pane the agent runs in: the terminal's first pane, the one `new-session` opened.
/// Not the active one: a pane the owner split off and left focused is not the agent.
fn agent_pane(tmux: &Server, name: &SessionName) -> Result<PaneId, Error> {
    agent_pane_info(tmux, name).map(|pane| pane.id)
}

/// [`agent_pane`] with its process's state.
pub(crate) fn agent_pane_info(tmux: &Server, name: &SessionName) -> Result<PaneInfo, Error> {
    let number = |pane: &PaneInfo| pane.id.as_str()[1..].parse::<u64>().unwrap_or(u64::MAX);
    tmux.panes(name)?
        .into_iter()
        .min_by_key(number)
        .ok_or_else(|| {
            Error::Tmux(cockpit_tmux::Error::UnexpectedOutput {
                arguments: Vec::new(),
                output: format!("terminal {name} has no pane"),
            })
        })
}

/// `environment` with every temporary file in `temp_dir`, a session's own (#766): Claude
/// Code reads its own variable; every other program, OpenCode and Codex included, follows
/// `TMPDIR`.
fn with_temp_dir(
    mut environment: Vec<(OsString, OsString)>,
    temp_dir: &Path,
) -> Vec<(OsString, OsString)> {
    environment.retain(|(name, _)| name != "TMPDIR" && name != "CLAUDE_CODE_TMPDIR");
    environment.push(("TMPDIR".into(), temp_dir.into()));
    environment.push(("CLAUDE_CODE_TMPDIR".into(), temp_dir.into()));
    environment
}

/// A launch's bound credentials file (#832), removed if the launch returns before its agent is
/// released, so a launch that fails before its launcher runs leaves none. Once released, the
/// launcher runs and removes it as it starts: removing it then would race the launcher, which
/// would find none and refuse the launch.
struct StagedCredentials(Option<PathBuf>);

impl StagedCredentials {
    /// The released launcher owns the file from now on.
    fn hand_over(&mut self) {
        self.0 = None;
    }
}

impl Drop for StagedCredentials {
    fn drop(&mut self) {
        let Some(path) = &self.0 else {
            return;
        };
        match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => tracing::warn!(
                path = %path.display(),
                %error,
                "a launch's credentials file could not be removed"
            ),
            _ => {}
        }
    }
}

/// What a launch in one project gets of GitHub (#754).
#[derive(Debug, Default)]
struct GithubAccess {
    /// The project's token, when the owner keeps one: the agent's `GH_TOKEN` when unsandboxed.
    /// It can push, so a sandboxed agent never gets it (#857).
    token: Option<cockpit_config::GithubToken>,
    /// The project's API token, when the owner keeps one and the launch is sandboxed: what the
    /// session proxy adds on GitHub's hosts, the agent holding a placeholder (#832).
    api_token: Option<cockpit_config::GithubToken>,
    /// The owner's own `gh` login, for the sandbox profile to hide from an agent that has its
    /// project's token, so `gh` and Git use that token alone. No backend enforces the profile
    /// in this release (#828), so an unsandboxed agent can still read it. Empty without a
    /// token: the owner decided agents may use their login until each project has one. A
    /// sandboxed agent's home is its session's own, which holds no login either way.
    hidden_owner_login: Vec<PathBuf>,
}

/// [`GithubAccess`] for a launch in `project` under `setting`, from the token files in the
/// owner's configuration; the API token's only under `"required"`, the one setting that uses
/// it. Blocking: it reads the files. An owner environment that names no home (in-process
/// tests) reads none.
fn github_access(
    environment: &cockpit_config::Environment,
    setting: cockpit_config::AgentSandbox,
    project: &str,
) -> Result<GithubAccess, Error> {
    let (Ok(home), Some(path), Some(api_path)) = (
        environment.home_dir(),
        cockpit_config::github_token_file(environment, project),
        cockpit_config::github_api_token_file(environment, project),
    ) else {
        return Ok(GithubAccess::default());
    };
    let read = |path: &Path| {
        cockpit_config::read_github_token(path).map_err(|error| Error::GithubToken {
            project: project.to_owned(),
            error,
        })
    };
    let token = read(&path)?;
    let api_token = match setting {
        cockpit_config::AgentSandbox::Required => read(&api_path)?,
        cockpit_config::AgentSandbox::Off => None,
    };
    let hidden_owner_login = match token {
        Some(_) => owner_gh_config_dirs(environment, &home),
        None => Vec::new(),
    };
    Ok(GithubAccess {
        token,
        api_token,
        hidden_owner_login,
    })
}

/// What the session proxy adds the project's API token as, on each of GitHub's hosts (#857):
/// Bearer on the API, Basic on Git's reads alone, a push refused naming `torlek push`.
const GITHUB_API_TOKEN_HOSTS: [(&str, GithubScheme); 2] = [
    ("github.com", GithubScheme::GitReads),
    ("api.github.com", GithubScheme::Bearer),
];

/// The name every refusal, report and evidence line gives the project's API token.
const GITHUB_API_TOKEN_NAME: &str = "the project's GitHub API token";

/// The username GitHub expects beside a token in Basic credentials.
const GITHUB_GIT_USERNAME: &str = "x-access-token";

#[derive(Clone, Copy)]
enum GithubScheme {
    GitReads,
    Bearer,
}

/// The project's API token, bound to the GitHub hosts a launch reaches.
struct GithubApiCredential {
    /// None when the launch reaches none of them.
    credential: Option<cockpit_sandbox::BoundCredential>,
    /// The hosts the launch does not reach, which get no credential.
    unbound: Vec<&'static str>,
}

impl GithubApiCredential {
    /// The evidence that names each host left without the token, so a `gh` or a fetch that
    /// fails there for want of it says why.
    fn unbound_rules(&self) -> Vec<cockpit_sandbox::AppliedRule> {
        self.unbound
            .iter()
            .map(|host| cockpit_sandbox::AppliedRule {
                rule: format!(
                    "credential: {GITHUB_API_TOKEN_NAME} is not added on {host}:443: the \
                     session's launch does not reach it, and a credential is bound at launch \
                     only to a host the launch reaches (#857), by its needs or the owner's \
                     grants and allows (#674); an allow made after the launch adds none until \
                     the session launches again"
                ),
                enforced_by: "torlek".to_owned(),
            })
            .collect()
    }
}

/// `api_token` bound to each of GitHub's hosts that `reach` lets through on 443: binding a
/// credential never grants reach (#857, decision 3), so a host the launch does not reach gets
/// none and is named instead.
fn github_api_credential(
    api_token: &cockpit_config::GithubToken,
    reach: &cockpit_sandbox::Allowlist,
) -> GithubApiCredential {
    let (reached, unbound): (Vec<_>, Vec<_>) = GITHUB_API_TOKEN_HOSTS
        .iter()
        .partition(|(host, _)| reach.decide(host, cockpit_sandbox::TLS_PORT).is_ok());
    let hosts: Vec<cockpit_sandbox::CredentialHost> = reached
        .iter()
        .map(|(host, scheme)| cockpit_sandbox::CredentialHost {
            host: (*host).to_owned(),
            scheme: match scheme {
                GithubScheme::GitReads => cockpit_sandbox::CredentialScheme::GitReads {
                    username: GITHUB_GIT_USERNAME.to_owned(),
                },
                GithubScheme::Bearer => cockpit_sandbox::CredentialScheme::Bearer,
            },
        })
        .collect();
    let credential = (!hosts.is_empty()).then(|| cockpit_sandbox::BoundCredential {
        name: GITHUB_API_TOKEN_NAME.to_owned(),
        placeholder_variable: Some(cockpit_config::GITHUB_TOKEN_VARIABLE.to_owned()),
        value: cockpit_sandbox::CredentialValue::new(api_token.expose().to_owned()),
        hosts,
    });
    GithubApiCredential {
        credential,
        unbound: unbound.iter().map(|(host, _)| *host).collect(),
    }
}

/// Where the owner's `gh` keeps its login (`hosts.yml`): `~/.config/gh`, which a provider's
/// profile may let every agent read, and `$XDG_CONFIG_HOME/gh`, where `gh` looks when it is set.
fn owner_gh_config_dirs(environment: &cockpit_config::Environment, home: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![home.join(".config").join("gh")];
    let xdg_config = environment
        .xdg_config_home
        .as_ref()
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("gh"));
    if let Some(dir) = xdg_config
        && !dirs.contains(&dir)
    {
        dirs.push(dir);
    }
    dirs
}

/// What `provider`'s adapter declares for the sandbox (#830, #858), with the registries of
/// the checkout's `ecosystems` in its egress when it declares any host (#859, #861); a
/// malformed declaration refuses the launch rather than narrowing it.
fn provider_needs(
    provider: &dyn Provider,
    ecosystems: &[String],
) -> Result<cockpit_sandbox::ProviderNeeds, Error> {
    let refused = |error: cockpit_provider_api::InvalidValue| {
        Error::Provider(ProviderError {
            reason: format!("its sandbox declaration is invalid: {error}"),
        })
    };
    let needs = provider.needs().map_err(refused)?;
    let credentials = provider.credentials().map_err(refused)?;
    let declares_nothing = needs.hosts.is_empty()
        && needs.read.is_empty()
        && needs.write.is_empty()
        && credentials.files.is_empty()
        && credentials.bindings.is_empty();
    let adapter = cockpit_sandbox::AllowSource::Adapter {
        provider: provider.name().as_str().to_owned(),
    };
    let mut egress: Vec<cockpit_sandbox::AllowEntry> = needs
        .hosts
        .iter()
        .map(|host| allow_entry(host.as_str(), adapter.clone()))
        .collect();
    if !egress.is_empty() {
        for ecosystem in ecosystems {
            let toolchain_refused = |reason: String| {
                Error::Provider(ProviderError {
                    reason: format!("the {ecosystem} toolchain {reason}"),
                })
            };
            let toolchain = cockpit_provider_api::toolchains::TOOLCHAINS
                .iter()
                .find(|toolchain| toolchain.ecosystem == ecosystem)
                .ok_or_else(|| toolchain_refused("is not in the toolchain table".to_owned()))?;
            let source = cockpit_sandbox::AllowSource::Ecosystem {
                ecosystem: ecosystem.clone(),
            };
            for host in toolchain.registry_hosts {
                let host = cockpit_provider_api::Host::new(*host).map_err(|error| {
                    toolchain_refused(format!("has an invalid registry host: {error}"))
                })?;
                egress.push(allow_entry(host.as_str(), source.clone()));
            }
        }
    }
    let source = if declares_nothing {
        cockpit_sandbox::NeedsSource::Floor
    } else {
        cockpit_sandbox::NeedsSource::Adapter {
            provider: provider.name().as_str().to_owned(),
        }
    };
    Ok(cockpit_sandbox::ProviderNeeds {
        source,
        read: needs
            .read
            .iter()
            .map(|path| path.as_str().to_owned())
            .collect(),
        credential_files: credentials
            .files
            .iter()
            .map(|path| path.as_str().to_owned())
            .collect(),
        credential_variables: credentials
            .bindings
            .into_iter()
            .map(|binding| binding.variable)
            .collect(),
        egress,
    })
}

/// A declared host, `name` or `name:port` as [`cockpit_provider_api::Host`] validated it,
/// as the proxy's entry: port 443 when it names none.
fn allow_entry(host: &str, source: cockpit_sandbox::AllowSource) -> cockpit_sandbox::AllowEntry {
    let (name, port) = match host.rsplit_once(':') {
        Some((name, port)) => (name, port.parse().unwrap_or(0)),
        None => (host, cockpit_sandbox::TLS_PORT),
    };
    cockpit_sandbox::AllowEntry {
        target: cockpit_sandbox::Target::Name(name.to_owned()),
        port,
        source,
    }
}

/// What the checkout at `checkout`, started in `start_dir`, and Git ask of a sandboxed
/// launch with `environment` (#859): the ecosystems the toolchain table detects at either
/// folder, the owner's install roots the launch's `PATH` resolves their tools in, every
/// cache variable of the table, so a tool of an ecosystem without a marker file still keeps
/// its cache in the session's own folder, and the owner's Git identity for the checkout.
fn tool_needs(
    checkout: &Path,
    start_dir: &Path,
    environment: &[(OsString, OsString)],
) -> Result<cockpit_sandbox::ToolNeeds, Error> {
    use cockpit_provider_api::toolchains::{COMMON_VARIABLES, TOOLCHAINS};

    let found = [
        detected_toolchains(checkout),
        detected_toolchains(start_dir),
    ]
    .concat();
    let detected: Vec<_> = TOOLCHAINS
        .iter()
        .filter(|toolchain| found.contains(toolchain))
        .collect();
    let path_entries: Vec<PathBuf> = launch_variable(environment, "PATH")
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    let mut install_roots: Vec<PathBuf> = Vec::new();
    // Without a HOME the sandbox refuses the launch itself.
    if let Some(home) = launch_variable(environment, "HOME") {
        for toolchain in &detected {
            for root in toolchain.roots_to_bind(Path::new(home), &path_entries)? {
                if !install_roots.contains(&root) {
                    install_roots.push(root);
                }
            }
        }
    }
    let mut variables: Vec<(String, cockpit_sandbox::ToolSetting)> = Vec::new();
    for (name, setting) in COMMON_VARIABLES
        .iter()
        .chain(TOOLCHAINS.iter().flat_map(|toolchain| toolchain.variables))
    {
        if !variables.iter().any(|(known, _)| known == name) {
            variables.push(((*name).to_owned(), tool_setting(*setting)));
        }
    }
    Ok(cockpit_sandbox::ToolNeeds {
        ecosystems: detected
            .iter()
            .map(|toolchain| toolchain.ecosystem.to_owned())
            .collect(),
        install_roots,
        variables,
        git_identity: owner_git_identity(checkout, environment),
    })
}

/// The toolchains whose marker files `folder` holds. A marker that cannot be read, such as a
/// link loop an agent left in its checkout, detects nothing and is logged: it must not make
/// every later launch fail, and an undetected toolchain only binds less.
fn detected_toolchains(folder: &Path) -> Vec<&'static cockpit_provider_api::toolchains::Toolchain> {
    cockpit_provider_api::toolchains::detect(folder).unwrap_or_else(|error| {
        tracing::warn!(
            folder = %folder.display(),
            %error,
            "a sandboxed launch detects no toolchain in this folder"
        );
        Vec::new()
    })
}

fn tool_setting(
    setting: cockpit_provider_api::toolchains::Setting,
) -> cockpit_sandbox::ToolSetting {
    use cockpit_provider_api::toolchains::Setting;

    match setting {
        Setting::CacheFolder(name) => cockpit_sandbox::ToolSetting::CacheFolder(name.to_owned()),
        Setting::ConfigFile(name) => cockpit_sandbox::ToolSetting::ConfigFile(name.to_owned()),
        Setting::Value(value) => cockpit_sandbox::ToolSetting::Value(value.to_owned()),
        Setting::Flag(flag) => cockpit_sandbox::ToolSetting::Flag(flag.to_owned()),
    }
}

/// The last value `environment` gives `name`, as the launched program sees it.
fn launch_variable<'a>(
    environment: &'a [(OsString, OsString)],
    name: &str,
) -> Option<&'a std::ffi::OsStr> {
    environment
        .iter()
        .rev()
        .find(|(variable, _)| variable == name)
        .map(|(_, value)| value.as_os_str())
}

/// The owner's `user.name` and `user.email` for `checkout`, read by one Git run with the
/// launch's own `HOME` from inside it, so an `includeIf` on the checkout's folder applies.
/// An unset or unreadable one is left out, and Git inside the sandbox then asks for it.
fn owner_git_identity(
    checkout: &Path,
    environment: &[(OsString, OsString)],
) -> cockpit_sandbox::GitIdentity {
    let mut identity = cockpit_sandbox::GitIdentity::default();
    let mut command = std::process::Command::new("git");
    command
        .env_clear()
        .arg("-C")
        .arg(checkout)
        .args([
            "config",
            "--global",
            "--includes",
            "--null",
            "--get-regexp",
            r"^user\.(name|email)$",
        ])
        .stdin(std::process::Stdio::null());
    for name in ["HOME", "PATH", "XDG_CONFIG_HOME", "GIT_CONFIG_GLOBAL"] {
        if let Some(value) = launch_variable(environment, name) {
            command.env(name, value);
        }
    }
    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            tracing::warn!(%error, "cannot run git to read the owner's identity for a sandboxed session");
            return identity;
        }
    };
    // Git exits 1 when no key matches.
    if output.status.code() == Some(1) {
        return identity;
    }
    if !output.status.success() {
        tracing::warn!(
            status = %output.status,
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "git could not read the owner's identity for a sandboxed session"
        );
        return identity;
    }
    // Each entry is its key, a newline and its value, ended by NUL; the last one of a key
    // wins, as in Git.
    for entry in output.stdout.split(|byte| *byte == 0) {
        let Ok(entry) = std::str::from_utf8(entry) else {
            tracing::warn!("an entry of the owner's Git identity is not UTF-8; it is left out");
            continue;
        };
        match entry.split_once('\n') {
            Some(("user.name", value)) => identity.name = Some(value.to_owned()),
            Some(("user.email", value)) => identity.email = Some(value.to_owned()),
            _ => {}
        }
    }
    identity
}

/// Removes `path` and everything in it. A tool may leave read-only folders, as Go's module
/// cache is unless `-modcacherw`, which the agent can unset (#859): when removal is denied,
/// the folders inside are made writable by their owner and removal is tried again.
fn remove_tree(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            make_folders_writable(path)?;
            std::fs::remove_dir_all(path)
        }
        other => other,
    }
}

/// Gives the owner write and search access to `folder` and every folder inside it; links
/// are not followed.
fn make_folders_writable(folder: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::symlink_metadata(folder)?;
    if !metadata.is_dir() {
        return Ok(());
    }
    let mode = metadata.permissions().mode();
    if mode & 0o700 != 0o700 {
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(mode | 0o700))?;
    }
    for entry in std::fs::read_dir(folder)? {
        make_folders_writable(&entry?.path())?;
    }
    Ok(())
}

/// `environment` with `token` as its only GitHub token (#754): none the daemon or a
/// provider passed reaches the agent, so a project without one never gets another's or the
/// owner's.
fn with_github_token(
    mut environment: Vec<(OsString, OsString)>,
    token: Option<&cockpit_config::GithubToken>,
) -> Vec<(OsString, OsString)> {
    environment.retain(|(name, _)| {
        !cockpit_config::GITHUB_TOKEN_VARIABLES
            .iter()
            .any(|variable| name == variable)
    });
    if let Some(token) = token {
        environment.push((
            cockpit_config::GITHUB_TOKEN_VARIABLE.into(),
            token.expose().into(),
        ));
    }
    environment
}

/// Creates `dir` and its parents, private to the owner. One that exists is kept and made
/// owner-only again: its agent, which writes it, may have widened it.
fn create_session_dir(dir: &Path) -> Result<(), Error> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let failed = |source| Error::Io {
        action: "create",
        path: dir.to_path_buf(),
        source,
    };
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(failed)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(failed)
}

/// The trust check and write of [`Sessions::trust_clone`], blocking: it reads the owner's
/// configuration, the clone's and its project's Git origins, and the harness's files.
fn record_clone_trust(
    provider: &dyn Provider,
    owner_environment: &cockpit_config::Environment,
    git: &Git,
    checkout: &Checkout,
    working_dir: &Path,
    environment: &[(OsString, OsString)],
) -> Result<WorkspaceTrust, String> {
    let config = cockpit_config::load(owner_environment, None, &[])
        .map_err(|error| format!("the owner configuration cannot be read: {error}"))?;
    let trusted_roots = cockpit_projects::resolve_trusted_roots(config.trusted_projects.value);
    let trusted_checkout = cockpit_projects::harness_trust_source(checkout, &trusted_roots, git)?;
    provider
        .trust_workspace(&TrustRequest {
            workspace: &checkout.path,
            working_dir,
            trusted_checkout: &trusted_checkout,
            environment,
        })
        .map_err(|error| error.reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_lock_nothing_holds_is_not_held() {
        let session = Arc::new(SessionLock::default());

        assert!(!session_lock_is_held(&session));
    }

    #[test]
    fn a_session_lock_an_operation_holds_is_held() {
        let session = Arc::new(SessionLock::default());
        let _operation = Arc::clone(&session);

        assert!(session_lock_is_held(&session));
    }

    #[test]
    fn a_session_lock_whose_mutex_alone_is_locked_is_held() {
        let session = Arc::new(SessionLock::default());
        let _guard = Arc::clone(&session.operation)
            .try_lock_owned()
            .expect("nothing else locks it");

        assert!(session_lock_is_held(&session));
    }

    /// A launch environment with `home` and nothing on `PATH` but `path_first`.
    fn tool_environment(home: &Path, path_first: &Path) -> Vec<(OsString, OsString)> {
        vec![
            ("HOME".into(), home.into()),
            ("PATH".into(), path_first.into()),
        ]
    }

    #[test]
    fn a_toolchain_whose_marker_is_in_the_start_folder_alone_is_detected() {
        let checkout = tempfile::tempdir().unwrap();
        let web = checkout.path().join("web");
        std::fs::create_dir(&web).unwrap();
        std::fs::write(web.join("package.json"), "{}\n").unwrap();
        let home = tempfile::tempdir().unwrap();
        let node = home.path().join(".nvm/versions/node/v20.0.0/bin");

        let tools =
            tool_needs(checkout.path(), &web, &tool_environment(home.path(), &node)).unwrap();

        assert_eq!(tools.ecosystems, vec!["node".to_owned()]);
        assert_eq!(
            tools.install_roots,
            vec![home.path().join(".nvm/versions/node/v20.0.0")]
        );
    }

    #[test]
    fn a_marker_link_loop_an_agent_left_detects_nothing_and_does_not_fail_the_launch() {
        let checkout = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("Cargo.toml.loop", checkout.path().join("Cargo.toml")).unwrap();
        std::os::unix::fs::symlink("Cargo.toml", checkout.path().join("Cargo.toml.loop")).unwrap();
        let home = tempfile::tempdir().unwrap();

        let tools = tool_needs(
            checkout.path(),
            checkout.path(),
            &tool_environment(home.path(), &home.path().join(".cargo/bin")),
        )
        .unwrap();

        assert!(tools.ecosystems.is_empty(), "{:?}", tools.ecosystems);
    }

    #[test]
    fn a_tree_with_read_only_folders_is_removed() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let cache = temp
            .path()
            .join("caches/go-mod/example.com/dependency@v1.0.0");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("go.mod"), "module example.com/dependency\n").unwrap();
        for folder in [cache.as_path(), cache.parent().unwrap()] {
            std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        let session = temp.path().join("caches");

        remove_tree(&session).unwrap();

        assert!(!session.exists());
    }
}
