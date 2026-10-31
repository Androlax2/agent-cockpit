//! Routes the session commands to `cockpit-sessions` and converts its types to the wire at
//! this boundary, and runs its periodic pass. The rules live in that crate; this file maps.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cockpit_protocol::{
    Command, ErrorCode, LaunchKindView, LaunchView, ProtocolError, Reply, SessionFailure,
    SessionStateView, SessionView, StopTargetView,
};
use cockpit_sessions::{
    Error, LaunchKind, Restored, Session, SessionId, SessionState, Sessions, StartRequest,
    StopTarget, WakeCause,
};

use cockpit_authorization::SessionId as TaskSession;
use cockpit_tasks::SessionWork;

use crate::approvals::{Answer, Failed, job};
use crate::routes::{Route, WorkClass, refusal};

/// The route for one of the session commands; `routes.rs` sends only those here. Listing
/// is a read and runs in the control lane; ending and sleeping only stop an agent, so they
/// run in the stop lane; every other command may launch an agent, so it runs in the bulk lane.
pub(crate) fn route(command: Command, sessions: Arc<Sessions>) -> Route {
    let class = match command {
        Command::SessionList { .. } => WorkClass::Control,
        Command::SessionEnd { .. } | Command::SessionSleep { .. } => WorkClass::Stop,
        _ => WorkClass::Bulk,
    };
    let job: crate::Job = match command {
        Command::SessionList { ended } => job(async move {
            let work = task_work(&sessions).await?;
            let mut views = Vec::new();
            for session in sessions.list(ended).await? {
                views.push(current_view(&sessions, &session, &work).await?);
            }
            Ok(Reply::Sessions { sessions: views })
        }),
        Command::SessionNew {
            dir,
            agent,
            prompt,
            clone,
        } => job(async move {
            let work = task_work(&sessions).await?;
            let started = sessions
                .start(StartRequest {
                    dir: dir.into(),
                    provider: agent,
                    first_message: prompt,
                    clone_branch: clone,
                })
                .await?;
            reply(&sessions, &started, &work).await
        }),
        Command::SessionRestartAll => job(async move {
            // Read before restarting: a failed read must not report a restart that ran.
            let work = task_work(&sessions).await?;
            let outcome = sessions.restart_all().await?;
            // A restarted agent was just launched on the current start files.
            let restarted = outcome
                .restarted
                .iter()
                .map(|session| session_view(session, Vec::new(), &work))
                .collect();
            let deferred = outcome
                .deferred
                .into_iter()
                .map(|(session, changed)| session_view(&session, changed, &work))
                .collect();
            let failed = outcome
                .failed
                .iter()
                .map(|(id, error)| SessionFailure {
                    session: id.to_string(),
                    error: protocol_error(error),
                })
                .collect();
            Ok(Reply::RestartedAll {
                restarted,
                deferred,
                failed,
            })
        }),
        Command::SessionRestart { session } => one(session, sessions, |sessions, id| async move {
            sessions.restart(&id).await
        }),
        Command::SessionSleep { session } => one(session, sessions, |sessions, id| async move {
            sessions.sleep(&id).await
        }),
        Command::SessionWake { session } => one(session, sessions, |sessions, id| async move {
            sessions.wake(&id, WakeCause::Owner).await
        }),
        Command::SessionResume { session } => one(session, sessions, |sessions, id| async move {
            sessions.resume(&id).await
        }),
        Command::SessionEnd { session } => one(session, sessions, |sessions, id| async move {
            sessions.end(&id).await
        }),
        Command::SessionEndFinished { only } => job(end_finished(sessions, only)),
        Command::SessionLink { session, url } => {
            one(session, sessions, |sessions, id| async move {
                sessions.link(&id, &url).await
            })
        }
        other => {
            tracing::error!(
                ?other,
                "a non-session command was routed to the session handler"
            );
            return Route::Answer(refusal(
                ErrorCode::Internal,
                "the daemon routed this command to the wrong handler",
            ));
        }
    };
    Route::Run { class, job }
}

/// The job for an operation on the one session `session` names.
fn one<Operation>(
    session: String,
    sessions: Arc<Sessions>,
    operation: impl FnOnce(Arc<Sessions>, SessionId) -> Operation + Send + 'static,
) -> crate::Job
where
    Operation: Future<Output = Result<Session, Error>> + Send + 'static,
{
    job(async move {
        let id = SessionId::parse(&session)?;
        // Read before the operation: a failed read must not report an operation that ran.
        let work = task_work(&sessions).await?;
        let changed = operation(Arc::clone(&sessions), id).await?;
        reply(&sessions, &changed, &work).await
    })
}

impl From<Error> for Failed {
    fn from(error: Error) -> Failed {
        Failed(protocol_error(&error))
    }
}

/// `session-end-finished`: ends each finished session that has not ended (of `only`, when
/// given), one at a time; one that cannot end is listed with why and does not stop the
/// others. Each is checked again just before it ends, so one that took an item while another
/// was ending is kept.
async fn end_finished(sessions: Arc<Sessions>, only: Option<Vec<String>>) -> Answer {
    let work = task_work(&sessions).await?;
    let candidates: Vec<Session> = sessions
        .list(false)
        .await?
        .into_iter()
        .filter(|session| {
            only.as_ref()
                .is_none_or(|only| only.iter().any(|id| id == session.id.as_str()))
                && is_finished(session, &work)
        })
        .collect();
    let mut ended = Vec::new();
    let mut failed = Vec::new();
    for session in candidates {
        let current = sessions.get(&session.id).await?;
        if !is_finished(&current, &task_work(&sessions).await?) {
            continue;
        }
        match sessions.end(&session.id).await {
            Ok(over) => ended.push(session_view(&over, Vec::new(), &work)),
            Err(error) => failed.push(SessionFailure {
                session: session.id.to_string(),
                error: protocol_error(&error),
            }),
        }
    }
    Ok(Reply::EndedFinished { ended, failed })
}

/// Each session's items, as the task list records them.
pub(crate) type Work = HashMap<TaskSession, SessionWork>;

pub(crate) async fn task_work(sessions: &Sessions) -> Result<Work, Failed> {
    let read = sessions.database().read().await.map_err(Error::from)?;
    Ok(cockpit_tasks::session_work(&read).await?)
}

/// The finished label (#157, decided in #201): the task list, never the agent's own state,
/// says the session's work is done. It is doing no item and finished at least one. A session
/// mid-launch, mid-stop, failed or ended is not labelled: the failed one needs the owner,
/// the ended one has nothing left to exit.
fn is_finished(session: &Session, work: &Work) -> bool {
    if matches!(
        session.state,
        SessionState::Launching
            | SessionState::Stopping { .. }
            | SessionState::Failed
            | SessionState::Ended
    ) {
        return false;
    }
    let id = TaskSession::new(session.id.as_str())
        .expect("a session id is never empty, so it is a valid task-list session id");
    work.get(&id)
        .is_some_and(|work| work.doing == 0 && work.finished > 0)
}

pub(crate) fn protocol_error(error: &Error) -> ProtocolError {
    let code = error_code(error);
    if code == ErrorCode::Internal {
        tracing::error!(%error, "a session command failed");
    }
    ProtocolError::new(code, error.to_string())
}

fn error_code(error: &Error) -> ErrorCode {
    match error {
        Error::InvalidSessionId { .. }
        | Error::InvalidLink { .. }
        | Error::InvalidValue(_)
        | Error::ProviderNeeded { .. } => ErrorCode::InvalidArgument,
        Error::UnknownSession { .. }
        | Error::UnknownProvider { .. }
        | Error::NotInProject { .. } => ErrorCode::NotFound,
        Error::WrongState { .. }
        | Error::CannotResume { .. }
        | Error::SleepRefused { .. }
        | Error::LaunchInProgress { .. }
        | Error::StopUnconfirmed { .. } => ErrorCode::Conflict,
        Error::Provider(_) | Error::TmuxUnavailable { .. } => ErrorCode::Refused,
        Error::Projects(error) => crate::projects::error_code(error),
        _ => ErrorCode::Internal,
    }
}

/// The restart contract's sessions step: reopen what was active, before accepting.
pub(crate) async fn restore(sessions: &Sessions) -> Result<(), Error> {
    let restored = sessions.restore().await?;
    for outcome in &restored {
        match outcome {
            Restored::StillRunning(id) => tracing::info!(session = %id, "a session still runs"),
            Restored::Reopened(id) => tracing::info!(session = %id, "reopened a session"),
            Restored::StopFinished(id, state) => {
                tracing::info!(session = %id, %state, "finished a session's stop");
            }
            Restored::StopNotFinished(id, reason) => {
                tracing::warn!(session = %id, reason, "a session's stop could not be finished");
            }
            Restored::Failed(id, reason) => {
                tracing::warn!(session = %id, reason, "a session could not be restored");
            }
        }
    }
    tracing::info!(sessions = restored.len(), "restored sessions");
    Ok(())
}

/// The sessions' periodic pass: exits, idle sleep, asleep expiry. Runs until aborted.
pub(crate) async fn tick_forever(sessions: Arc<Sessions>, interval: Duration) {
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        match sessions.tick().await {
            Ok(report) => {
                for id in &report.exited {
                    tracing::info!(session = %id, "a session's agent exited; it ended");
                }
                for id in &report.slept {
                    tracing::info!(session = %id, "an idle session went to sleep");
                }
                for id in &report.expired {
                    tracing::info!(session = %id, "an asleep session expired; it ended");
                }
                for id in &report.restarted {
                    tracing::info!(session = %id, "a deferred restart ran at the end of a turn");
                }
                for (id, reason) in &report.errors {
                    tracing::warn!(session = %id, reason, "a session's periodic step failed");
                }
            }
            Err(error) => tracing::error!(%error, "the sessions' periodic pass failed"),
        }
    }
}

pub(crate) async fn session_reply(sessions: &Sessions, session: &Session) -> Answer {
    let work = task_work(sessions).await?;
    reply(sessions, session, &work).await
}

async fn reply(sessions: &Sessions, session: &Session, work: &Work) -> Answer {
    Ok(Reply::Session {
        session: Box::new(current_view(sessions, session, work).await?),
    })
}

/// The session's view with the start files that changed since its launch, compared now.
pub(crate) async fn current_view(
    sessions: &Sessions,
    session: &Session,
    work: &Work,
) -> Result<SessionView, Error> {
    let changed = sessions.changed_start_files(session).await?;
    Ok(session_view(session, changed, work))
}

fn session_view(session: &Session, changed_start_files: Vec<String>, work: &Work) -> SessionView {
    SessionView {
        id: session.id.to_string(),
        agent: session.provider.to_string(),
        checkout: session.checkout.0,
        holds_checkout: session.holds_checkout,
        working_dir: session.working_dir.to_string_lossy().into_owned(),
        conversation: session.conversation.as_ref().map(ToString::to_string),
        design_link: session.design_link.clone(),
        state: match session.state {
            SessionState::Launching => SessionStateView::Launching,
            SessionState::Running => SessionStateView::Running,
            SessionState::Stopping { to } => SessionStateView::Stopping {
                to: match to {
                    StopTarget::Asleep => StopTargetView::Asleep,
                    StopTarget::Ended => StopTargetView::Ended,
                    StopTarget::Restart => StopTargetView::Restart,
                },
            },
            SessionState::Asleep => SessionStateView::Asleep,
            SessionState::Ended => SessionStateView::Ended,
            SessionState::Failed => SessionStateView::Failed,
        },
        reason: session.reason.clone(),
        revision: session.revision,
        created_at: session.created_at,
        changed_at: session.changed_at,
        launch: session.launch.as_ref().map(|launch| LaunchView {
            number: launch.number,
            kind: match launch.kind {
                LaunchKind::Start => LaunchKindView::Start,
                LaunchKind::Restart => LaunchKindView::Restart,
                LaunchKind::Wake => LaunchKindView::Wake,
                LaunchKind::Resume => LaunchKindView::Resume,
                LaunchKind::Restore => LaunchKindView::Restore,
            },
            launched_at: launch.launched_at,
        }),
        changed_start_files,
        restart_pending: session.restart_pending,
        finished: is_finished(session, work),
    }
}
