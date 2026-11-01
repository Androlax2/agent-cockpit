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
use cockpit_provider_api::{Capability, Conversation, LaunchRequest, Provider, ProviderError};
use cockpit_tmux::{Server, SessionLaunch, SessionName};
use cockpit_turso::Database;

use crate::model::{LaunchPurpose, validate_link};
use crate::store::{self, NewSession};
use crate::{
    AgentState, Error, LaunchKind, RestartAll, Restored, Session, SessionId, SessionReport,
    SessionState, Settings, SleepRefusal, StartRequest, StopTarget, TickReport, WakeCause,
};

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
    processes: Supervisor,
    /// Cockpit's tmux server, which the caller opens and may share, or why it could not be
    /// opened: the daemon starts without tmux (report 09), and each operation that needs a
    /// terminal refuses with that reason.
    tmux: Result<Arc<Server>, String>,
    git: Git,
    providers: Vec<Arc<dyn Provider>>,
    settings: Settings,
    /// One lock per session, held by each owner operation and each periodic step on it, so
    /// the periodic pass never acts on a launch an owner operation just replaced. Entries
    /// stay for the daemon's life: one per session ever touched.
    operations: Mutex<HashMap<SessionId, Arc<SessionLock>>>,
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
    pub fn new(
        database: Arc<Database>,
        processes: Supervisor,
        tmux: Result<Arc<Server>, String>,
        git: Git,
        providers: Vec<Arc<dyn Provider>>,
        settings: Settings,
    ) -> Sessions {
        assert!(
            settings.blocking_steps > 0,
            "the sessions need at least one blocking step permit"
        );
        Sessions {
            blocking_permits: Arc::new(tokio::sync::Semaphore::new(settings.blocking_steps)),
            database,
            processes,
            tmux,
            git,
            providers,
            desktop_environment: Mutex::new(Vec::new()),
            owner_settings: Mutex::new(settings.owner),
            settings,
            operations: Mutex::new(HashMap::new()),
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

    /// Whether launched agents get the owner's desktop session variables (#305).
    pub fn desktop_session(&self) -> crate::DesktopSession {
        self.owner_settings().desktop_session
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
            },
        )
        .await?;
        if holds_checkout {
            cockpit_projects::claim(&write, checkout.id, &holder(&id)).await?;
        }
        write.commit().await?;
        tracing::info!(session = %id, provider = %provider.name(), "starting a session");
        self.launch(
            &operation,
            &id,
            LaunchKind::Start,
            LaunchPurpose::Work,
            Conversation::New,
            request.first_message.as_deref(),
        )
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
        self.resumable(&session)?;
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
        self.resumable(&session)?;
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
        let conversation = self.resumable(&session)?;
        // Waking reopens the conversation as resume does, so it is refused the same way.
        self.refuse_if_agent_alive_elsewhere(&session).await?;
        let write = self.database.write().await?;
        refuse_if_active_elsewhere(&write, &session).await?;
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
        let conversation = self.resumable(&session)?;
        self.refuse_if_agent_alive_elsewhere(&session).await?;
        if matches!(session.state, SessionState::Ended | SessionState::Failed) {
            self.stop_previous_agent(&session).await?;
        }
        let write = self.database.write().await?;
        refuse_if_active_elsewhere(&write, &session).await?;
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
        self.launch(
            &operation,
            id,
            LaunchKind::Resume,
            LaunchPurpose::Work,
            conversation,
            None,
        )
        .await
    }

    /// Refuses to launch `session`'s conversation while another session's agent of it is
    /// still alive, as `cockpit-processes` observes its execution now (report 04: an
    /// execution is identified by its supervisor identity, never a PID): a failed launch
    /// may have left one running whatever its session's state says.
    async fn refuse_if_agent_alive_elsewhere(&self, session: &Session) -> Result<(), Error> {
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
                    reason: format!(
                        "its conversation is still alive as session {}'s agent (execution {})",
                        holder.id, launch.execution.0
                    ),
                });
            }
        }
        Ok(())
    }

    /// Ends the session: stops its agent, releases the clone it occupies. A launch or a
    /// stop left unfinished by an error is stopped as an end. A failed launch may have left
    /// its agent running (#221), so a failed session's latest agent is stopped first, as
    /// resume does; one not observed gone refuses the end and the session stays failed.
    pub async fn end(&self, id: &SessionId) -> Result<Session, Error> {
        let operation = self.lock_to_stop(id, "end").await?;
        let session = self.get(id).await?;
        if matches!(
            session.state,
            SessionState::Running | SessionState::Launching | SessionState::Stopping { .. }
        ) {
            self.tmux()?;
            return self
                .stop(&operation, id, &[session.state], StopTarget::Ended, "end")
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
            Some("ended by the owner"),
            "end",
        )
        .await?;
        give_up_on_end(&write, &ended).await?;
        write.commit().await?;
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
        let stopped = match self.finish_stop(&id, to).await {
            Ok(stopped) => stopped,
            Err(error) if error.is_database() => return Err(error),
            Err(error) => return Ok(Restored::StopNotFinished(id, error.to_string())),
        };
        if to != StopTarget::Restart {
            return Ok(Restored::StopFinished(id, stopped.state));
        }
        // Who asked for the interrupted restart is not kept: a working one ([`LaunchPurpose`]).
        match self
            .relaunch(&operation, &stopped, LaunchPurpose::Work)
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
        let conversation = match self.resumable(session) {
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
        let (running, asleep) = {
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
            )
        };
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
            match self.end_if_exited(&session).await {
                Ok(true) => report.exited.push(id),
                Ok(false) => {
                    let agent_report = agent_report.as_ref();
                    if session.restart_pending && turn_refusal(&session, agent_report).is_none() {
                        match self.restart_locked(&operation, &id).await {
                            Ok(_) => report.restarted.push(id),
                            Err(error) => report.errors.push((id, error.to_string())),
                        }
                        continue;
                    }
                    if self.idle_sleep_refusal(&session, agent_report).is_some() {
                        continue;
                    }
                    match self
                        .stop(
                            &operation,
                            &id,
                            &[SessionState::Running],
                            StopTarget::Asleep,
                            "sleep",
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
        Ok(report)
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
        Ok(true)
    }

    /// Ends a running session whose agent was observed gone; `true` when it did.
    async fn end_if_exited(&self, session: &Session) -> Result<bool, Error> {
        let Some(launch) = &session.launch else {
            return Ok(false);
        };
        let execution = self
            .processes
            .observe(&self.database, launch.execution)
            .await?;
        if execution.phase.is_occupying() {
            return Ok(false);
        }
        self.close_terminal(&session.id).await?;
        let write = self.database.write().await?;
        let ended = store::transition(
            &write,
            &session.id,
            &[SessionState::Running],
            SessionState::Ended,
            Some("its agent exited; resume reopens it"),
            "end",
        )
        .await?;
        give_up_on_end(&write, &ended).await?;
        write.commit().await?;
        tracing::info!(session = %session.id, "a session's agent exited");
        Ok(true)
    }

    /// Why the owner may not put `session` to sleep: it is not running, or could not be
    /// woken.
    fn sleep_refusal(&self, session: &Session) -> Option<SleepRefusal> {
        if session.state != SessionState::Running {
            return Some(SleepRefusal::NotRunning(session.state));
        }
        if let Err(error) = self.resumable(session) {
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

    /// The conversation to launch again, or why there is none.
    fn resumable(&self, session: &Session) -> Result<Conversation, Error> {
        let cannot = |reason: &str| Error::CannotResume {
            session: session.id.clone(),
            reason: reason.to_owned(),
        };
        let provider = self.provider(Some(session.provider.as_str()))?;
        if !provider.capabilities().supports(Capability::Resume) {
            return Err(cannot("its agent's provider cannot resume a conversation"));
        }
        let conversation = session.conversation.clone().ok_or_else(|| {
            cannot("its provider never named the conversation; start a new session")
        })?;
        Ok(Conversation::Resume(conversation))
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
    async fn stop(
        &self,
        _lock: &Operation,
        id: &SessionId,
        from: &[SessionState],
        to: StopTarget,
        operation: &'static str,
    ) -> Result<Session, Error> {
        assert!(
            to != StopTarget::Restart,
            "a restart goes through restart_from, which relaunches it"
        );
        self.transition(id, from, SessionState::Stopping { to }, None, operation)
            .await?;
        self.finish_stop(id, to).await
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
        let stopped = self.finish_stop(id, to).await?;
        self.relaunch(lock, &stopped, purpose).await
    }

    /// Stops a stopping session's agent and moves it on: asleep, ended, or `launching` for
    /// a restart, which [`Self::relaunch`] then launches. An agent not observed gone puts
    /// the session back to running.
    async fn finish_stop(&self, id: &SessionId, to: StopTarget) -> Result<Session, Error> {
        let session = self.get(id).await?;
        let stopping = SessionState::Stopping { to };
        if let Err(error) = self.stop_previous_agent(&session).await {
            if let Error::StopUnconfirmed { execution, .. } = &error {
                tracing::warn!(
                    session = %id,
                    execution = execution.0,
                    "a session's agent did not stop after SIGKILL"
                );
                self.transition(
                    id,
                    &[stopping],
                    SessionState::Running,
                    Some("its agent did not stop after SIGKILL"),
                    "stop",
                )
                .await?;
            }
            return Err(error);
        }
        let (state, reason) = match to {
            StopTarget::Asleep => (SessionState::Asleep, None),
            StopTarget::Ended => (SessionState::Ended, Some("ended by the owner")),
            StopTarget::Restart => (SessionState::Launching, None),
        };
        let write = self.database.write().await?;
        let stopped = store::transition(&write, id, &[stopping], state, reason, "stop").await?;
        if to == StopTarget::Ended {
            give_up_on_end(&write, &stopped).await?;
        }
        write.commit().await?;
        Ok(stopped)
    }

    /// Launches a restart's conversation again once its stop finished. A failure leaves the
    /// session `failed` with the reason, and is returned.
    async fn relaunch(
        &self,
        lock: &Operation,
        stopped: &Session,
        purpose: LaunchPurpose,
    ) -> Result<Session, Error> {
        let conversation = match self.resumable(stopped) {
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
            if termination == Termination::Unconfirmed {
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
        let command = {
            let provider = Arc::clone(self.provider(Some(session.provider.as_str()))?);
            let working_dir = session.working_dir.clone();
            let conversation = conversation.clone();
            let first_message = first_message.map(str::to_owned);
            // Off the async workers and bounded: the contract says it does not block, but
            // a provider that does would otherwise hold the launch forever (#272).
            self.blocking("build the provider's command", move || {
                Ok(provider.launch(&LaunchRequest {
                    working_dir: &working_dir,
                    conversation: &conversation,
                    first_message: first_message.as_deref(),
                    effort: None,
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
        // The execution is reserved in the transaction that records the launch, so a crash
        // never leaves one the session does not name (#256).
        let (token, execution) = {
            let write = self.database.write().await?;
            let token = cockpit_agent_channel::mint(&write, &channel_session).await?;
            grant_self_permissions(&write, &channel_session).await?;
            let execution = self
                .processes
                .reserve(&write, self.database.epoch())
                .await?;
            store::insert_launch(&write, id, kind, purpose, execution, &start_files).await?;
            write.commit().await?;
            (token, execution)
        };
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
        environment.extend(command.environment);
        environment.push(("COCKPIT_SESSION".into(), id.as_str().into()));
        environment.push(("COCKPIT_LAUNCH_TOKEN".into(), token.as_str().into()));
        let spec = LaunchSpec {
            program: command.program,
            arguments: command.arguments,
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

    async fn confirm_agent(
        &self,
        session: &cockpit_authorization::SessionId,
        execution: ExecutionId,
    ) -> Result<cockpit_processes::Execution, Error> {
        let execution = self
            .processes
            .confirm_hosted(&self.database, execution)
            .await?;
        self.record_agent_root(session, execution.process_group)
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
            Ok(Termination::Unconfirmed) => tracing::warn!(
                session = %id,
                execution = execution.0,
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

/// What an ended session gives up, in the transaction that ends it: its clone and its self
/// grants. Every move to `ended` calls this.
async fn give_up_on_end(
    write: &cockpit_turso::WriteTransaction<'_>,
    ended: &Session,
) -> Result<(), Error> {
    release(write, ended).await?;
    store::clear_restart_request(write, &ended.id).await?;
    revoke_self_grants(write, &ended.id).await
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
