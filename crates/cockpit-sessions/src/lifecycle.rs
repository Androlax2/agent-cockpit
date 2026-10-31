//! The lifecycle operations: every transition goes through the stored state first, so two
//! operations on one session cannot interleave and a daemon that stopped midway finishes or
//! repairs the transition at its next start ([`Sessions::restore`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cockpit_authorization::{Permission, Scope};
use cockpit_config::StartFileSnapshot;
use cockpit_processes::{ExecutionId, LaunchSpec, Phase, Supervisor, Termination};
use cockpit_projects::{Checkout, CheckoutKind, Git, Holder};
use cockpit_provider_api::{Capability, Conversation, LaunchRequest, Provider, ProviderError};
use cockpit_tmux::{Server, SessionLaunch, SessionName};
use cockpit_turso::Database;

use crate::model::validate_link;
use crate::store::{self, NewSession};
use crate::{
    AgentState, Error, LaunchKind, RestartAll, Restored, Session, SessionId, SessionReport,
    SessionState, Settings, SleepRefusal, StartRequest, StopTarget, TickReport, WakeCause,
};

/// Variables of the terminal tmux emulates, kept from the pane: they describe the host.
const TERMINAL_VARIABLES: [&str; 2] = ["TERM", "COLORTERM"];

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
    operations: Mutex<HashMap<SessionId, Arc<tokio::sync::Mutex<()>>>>,
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
        Sessions {
            database,
            processes,
            tmux,
            git,
            providers,
            settings,
            operations: Mutex::new(HashMap::new()),
        }
    }

    /// The state database the sessions live in, for the daemon's views that join them with
    /// other capabilities' tables (a session's task work, #157).
    pub fn database(&self) -> &Arc<Database> {
        &self.database
    }

    async fn lock(&self, id: &SessionId) -> tokio::sync::OwnedMutexGuard<()> {
        let session_lock = {
            let mut operations = self
                .operations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(operations.entry(id.clone()).or_default())
        };
        session_lock.lock_owned().await
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
        let dir = canonical_dir(request.dir).await?;
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
        let _operation = self.lock(&id).await;
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
            &id,
            LaunchKind::Start,
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
        let _operation = self.lock(id).await;
        let session = self.get(id).await?;
        self.resumable(&session)?;
        self.stop(id, &RESTARTABLE, StopTarget::Restart, "restart")
            .await
    }

    /// Restarts a running session's agent; the caller holds its lock.
    async fn restart_locked(&self, id: &SessionId) -> Result<Session, Error> {
        self.stop(id, &[SessionState::Running], StopTarget::Restart, "restart")
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
        let _operation = self.lock(id).await;
        let Some((session, agent_report)) = self.running_with_report(id).await? else {
            return Ok(RestartStep::Current);
        };
        let changed = self.changed_start_files(&session).await?;
        if changed.is_empty() {
            return Ok(RestartStep::Current);
        }
        self.resumable(&session)?;
        if turn_refusal(&session, agent_report.as_ref()).is_none() {
            return Ok(RestartStep::Restarted(self.restart_locked(id).await?));
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
        blocking(move || {
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
        let _operation = self.lock(id).await;
        let session = self.get(id).await?;
        if let Some(refusal) = self.sleep_refusal(&session) {
            return Err(Error::SleepRefused {
                session: id.clone(),
                refusal,
            });
        }
        self.stop(id, &[SessionState::Running], StopTarget::Asleep, "sleep")
            .await
    }

    /// Launches an asleep session's conversation again: the one path every wake (owner,
    /// open, message, todo, phone) takes; `cause` is kept in its transition history. A
    /// session that is not asleep is refused.
    pub async fn wake(&self, id: &SessionId, cause: WakeCause) -> Result<Session, Error> {
        self.tmux()?;
        let _operation = self.lock(id).await;
        self.wake_locked(id, cause).await
    }

    /// For callers that need the agent running, whatever its state now (opening its
    /// terminal, delivering a message or a todo): wakes an asleep session and answers a
    /// launching or running one as it is. Any other state is refused, as by [`Self::wake`].
    pub async fn ensure_awake(&self, id: &SessionId, cause: WakeCause) -> Result<Session, Error> {
        let _operation = self.lock(id).await;
        let session = self.get(id).await?;
        if matches!(
            session.state,
            SessionState::Running | SessionState::Launching
        ) {
            return Ok(session);
        }
        self.tmux()?;
        self.wake_locked(id, cause).await
    }

    async fn wake_locked(&self, id: &SessionId, cause: WakeCause) -> Result<Session, Error> {
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
        self.launch(id, LaunchKind::Wake, conversation, None).await
    }

    /// Reopens an ended or failed session's conversation. A failed launch may have left its
    /// agent running, so the previous agent is stopped first. Refused while the conversation
    /// still runs in another session (#157).
    pub async fn resume(&self, id: &SessionId) -> Result<Session, Error> {
        self.tmux()?;
        let _operation = self.lock(id).await;
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
        self.launch(id, LaunchKind::Resume, conversation, None)
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
        let _operation = self.lock(id).await;
        let session = self.get(id).await?;
        if matches!(
            session.state,
            SessionState::Running | SessionState::Launching | SessionState::Stopping { .. }
        ) {
            self.tmux()?;
            return self
                .stop(id, &[session.state], StopTarget::Ended, "end")
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
    /// `failed` with the reason and is not retried. Stops under way are finished.
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
        let mut restored = Vec::new();
        for session in active {
            let id = session.id.clone();
            let outcome = match session.state {
                SessionState::Stopping {
                    to: to @ (StopTarget::Asleep | StopTarget::Ended),
                } => match self.finish_stop(&id, to).await {
                    Ok(stopped) => Restored::StopFinished(id, stopped.state),
                    Err(error) => Restored::StopNotFinished(id, error.to_string()),
                },
                _ => match self.restore_one(&session).await {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        self.fail_restore(&session, format!("restore failed: {error}"))
                            .await?
                    }
                },
            };
            restored.push(outcome);
        }
        Ok(restored)
    }

    async fn restore_one(&self, session: &Session) -> Result<Restored, Error> {
        let id = &session.id;
        if self.agent_is_running(session).await? {
            if session.state != SessionState::Running {
                self.transition(id, &[session.state], SessionState::Running, None, "restore")
                    .await?;
            }
            return Ok(Restored::StillRunning(id.clone()));
        }
        if let Err(error) = self.stop_previous_agent(session).await {
            return self
                .fail_restore(session, format!("restore refused: {error}"))
                .await;
        }
        if session.state != SessionState::Launching {
            self.transition(
                id,
                &[session.state],
                SessionState::Launching,
                None,
                "restore",
            )
            .await?;
        }
        let conversation = match self.resumable(session) {
            Ok(conversation) => conversation,
            Err(error) => {
                return self
                    .fail_restore(session, format!("restore failed: {error}"))
                    .await;
            }
        };
        Ok(
            match self
                .launch(id, LaunchKind::Restore, conversation, None)
                .await
            {
                Ok(_) => Restored::Reopened(id.clone()),
                Err(error) => Restored::Failed(id.clone(), format!("restore failed: {error}")),
            },
        )
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
            let _operation = self.lock(&id).await;
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
                        match self.restart_locked(&id).await {
                            Ok(_) => report.restarted.push(id),
                            Err(error) => report.errors.push((id, error.to_string())),
                        }
                        continue;
                    }
                    if self.idle_sleep_refusal(&session, agent_report).is_some() {
                        continue;
                    }
                    match self
                        .stop(&id, &[SessionState::Running], StopTarget::Asleep, "sleep")
                        .await
                    {
                        Ok(_) => report.slept.push(id),
                        Err(error) => report.errors.push((id, error.to_string())),
                    }
                }
                Err(error) => report.errors.push((id, error.to_string())),
            }
        }
        let expiry = i64::try_from(self.settings.asleep_expiry.as_secs()).unwrap_or(i64::MAX);
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
        let window = i64::try_from(self.settings.idle_sleep_after.as_secs()).unwrap_or(i64::MAX);
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

    /// Moves the session from one of `from` to `failed` and revokes its self grant: a failed
    /// launch may have left an agent running, and it must not report for the session.
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
        revoke_self_grant(&write, id).await?;
        store::clear_restart_request(&write, id).await?;
        write.commit().await?;
        Ok(failed)
    }

    /// Moves the session from one of `from` to stopping, then stops its agent.
    async fn stop(
        &self,
        id: &SessionId,
        from: &[SessionState],
        to: StopTarget,
        operation: &'static str,
    ) -> Result<Session, Error> {
        self.transition(id, from, SessionState::Stopping { to }, None, operation)
            .await?;
        self.finish_stop(id, to).await
    }

    /// Stops a stopping session's agent and moves it on: asleep, ended, or a launch of the
    /// same conversation. An agent not observed gone puts the session back to running.
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
        if to != StopTarget::Restart {
            return Ok(stopped);
        }
        let conversation = match self.resumable(&stopped) {
            Ok(conversation) => conversation,
            Err(error) => {
                self.fail(id, &error.to_string()).await?;
                return Err(error);
            }
        };
        self.launch(id, LaunchKind::Restart, conversation, None)
            .await
    }

    /// Stops the agent of the session's latest launch, if any, and closes its terminal;
    /// refused while that agent may still run.
    async fn stop_previous_agent(&self, session: &Session) -> Result<(), Error> {
        if let Some(launch) = &session.launch {
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

    /// Launches `conversation` for a session already `launching`. A failure leaves the
    /// session `failed` with the reason, and is returned.
    async fn launch(
        &self,
        id: &SessionId,
        kind: LaunchKind,
        conversation: Conversation,
        first_message: Option<&str>,
    ) -> Result<Session, Error> {
        match self
            .try_launch(id, kind, &conversation, first_message)
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
        conversation: &Conversation,
        first_message: Option<&str>,
    ) -> Result<Session, Error> {
        let tmux = self.tmux()?;
        let session = self.get(id).await?;
        let provider = self.provider(Some(session.provider.as_str()))?;
        let command = provider.launch(&LaunchRequest {
            working_dir: &session.working_dir,
            conversation,
            first_message,
            effort: None,
            remote_control: false,
        })?;
        if let Conversation::Resume(requested) = conversation
            && command.conversation.as_ref() != Some(requested)
        {
            return Err(Error::Provider(ProviderError {
                reason: format!("it did not resume conversation {requested}"),
            }));
        }
        let channel_session = authorization_session(id)?;
        let token = {
            let write = self.database.write().await?;
            let token = cockpit_agent_channel::mint(&write, &channel_session).await?;
            cockpit_authorization::grant(
                &write,
                &channel_session,
                Permission::ChangeState,
                &Scope::Session(channel_session.clone()),
            )
            .await?;
            write.commit().await?;
            token
        };
        let mut environment = self.settings.base_environment.clone();
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
        // Taken before the agent runs: a file changed between the two reads as changed.
        let start_files = {
            let declared = self.settings.start_files.clone();
            let provider = session.provider.as_str().to_owned();
            blocking(move || Ok(StartFileSnapshot::capture(&declared, &provider))).await?
        };
        let hosted = self
            .processes
            .prepare_hosted(&self.database, &spec, &TERMINAL_VARIABLES)
            .await?;
        let recorded = async {
            let write = self.database.write().await?;
            store::insert_launch(&write, id, kind, hosted.execution, &start_files).await?;
            write.commit().await?;
            Ok::<(), Error>(())
        };
        if let Err(error) = recorded.await {
            self.processes
                .abandon_hosted(&self.database, hosted.execution)
                .await?;
            return Err(error);
        }
        let terminal = SessionLaunch {
            name: terminal_name(id)?,
            working_dir: session.working_dir.clone(),
            command: hosted.command,
            environment: Vec::new(),
            columns: self.settings.columns,
            rows: self.settings.rows,
        };
        let opened = blocking(move || {
            close_if_open(&tmux, &terminal.name)?;
            tmux.new_session(&terminal).map(drop).map_err(Error::from)
        })
        .await;
        if let Err(error) = opened {
            self.processes
                .abandon_hosted(&self.database, hosted.execution)
                .await?;
            return Err(error);
        }
        let execution = match self.confirm_agent(&channel_session, hosted.execution).await {
            Ok(execution) => execution,
            Err(error) => {
                self.discard_launch(id, hosted.execution).await;
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
        tracing::info!(session = %id, ?kind, execution = execution.id.0, "a session runs");
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
        Ok(execution)
    }

    /// Undoes a launch whose terminal opened but whose agent could not be confirmed:
    /// closing the terminal hangs up the pane, then the execution is released or stopped.
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
        let identity = blocking(move || Ok(cockpit_agent_channel::identity_of(pid))).await?;
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
        blocking(move || Ok(tmux.sessions()?.contains(&name))).await
    }

    async fn close_terminal(&self, id: &SessionId) -> Result<(), Error> {
        let tmux = self.tmux()?;
        let name = terminal_name(id)?;
        blocking(move || close_if_open(&tmux, &name)).await
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
/// grant. Every move to `ended` calls this.
async fn give_up_on_end(
    write: &cockpit_turso::WriteTransaction<'_>,
    ended: &Session,
) -> Result<(), Error> {
    release(write, ended).await?;
    store::clear_restart_request(write, &ended.id).await?;
    revoke_self_grant(write, &ended.id).await
}

/// Revokes the `change-state` over itself a launch granted the session, so its agent may
/// no longer report. An owner grant of the same permission and scope is the same grant and
/// goes with it. Each revocation advances the authorization revision.
async fn revoke_self_grant(
    write: &cockpit_turso::WriteTransaction<'_>,
    id: &SessionId,
) -> Result<(), Error> {
    let session = authorization_session(id)?;
    let own_state = Scope::Session(session.clone());
    let self_grant = cockpit_authorization::active_grants(write, &session)
        .await?
        .into_iter()
        .find(|grant| grant.permission == Permission::ChangeState && grant.scope == own_state);
    if let Some(grant) = self_grant {
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

async fn canonical_dir(dir: PathBuf) -> Result<PathBuf, Error> {
    blocking(move || {
        let canonical = dir.canonicalize().map_err(|source| Error::Io {
            action: "resolve",
            path: dir.clone(),
            source,
        })?;
        if !canonical.is_dir() {
            return Err(Error::NotInProject { dir: canonical });
        }
        Ok(canonical)
    })
    .await
}

/// Runs a blocking tmux or filesystem step off the async workers and waits for it.
async fn blocking<T: Send + 'static>(
    step: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(step)
        .await
        .map_err(|error| Error::Blocking(error.to_string()))?
}
