//! Routes the session commands to `cockpit-sessions` and converts its types to the wire at
//! this boundary, and runs its periodic pass. The rules live in that crate; this file maps.

use std::sync::Arc;
use std::time::Duration;

use cockpit_protocol::{
    Command, ErrorCode, LaunchKindView, LaunchView, Outcome, ProtocolError, Reply, SessionFailure,
    SessionStateView, SessionView, StopTargetView,
};
use cockpit_sessions::{
    Error, LaunchKind, Restored, Session, SessionId, SessionState, Sessions, StartRequest,
    StopTarget, WakeCause,
};

use crate::routes::{Route, WorkClass, refusal};

/// The route for one of the session commands; `routes.rs` sends only those here. Listing
/// is a read and runs in the control lane; every other command may launch or stop an agent,
/// so it runs in the bulk lane.
pub(crate) fn route(command: Command, sessions: Arc<Sessions>) -> Route {
    let class = match command {
        Command::SessionList { .. } => WorkClass::Control,
        _ => WorkClass::Bulk,
    };
    let job: crate::Job = match command {
        Command::SessionList { ended } => job(async move {
            let mut views = Vec::new();
            for session in sessions.list(ended).await? {
                views.push(current_view(&sessions, &session).await?);
            }
            Ok(Reply::Sessions { sessions: views })
        }),
        Command::SessionNew {
            dir,
            agent,
            prompt,
            clone,
        } => job(async move {
            let started = sessions
                .start(StartRequest {
                    dir: dir.into(),
                    provider: agent,
                    first_message: prompt,
                    clone_branch: clone,
                })
                .await?;
            session_reply(&sessions, &started).await
        }),
        Command::SessionRestartAll => job(async move {
            let outcome = sessions.restart_all().await?;
            // A restarted agent was just launched on the current start files.
            let restarted = outcome
                .restarted
                .iter()
                .map(|session| session_view(session, Vec::new()))
                .collect();
            let deferred = outcome
                .deferred
                .into_iter()
                .map(|(session, changed)| session_view(&session, changed))
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
        let changed = operation(Arc::clone(&sessions), id).await?;
        session_reply(&sessions, &changed).await
    })
}

fn job(work: impl Future<Output = Result<Reply, Error>> + Send + 'static) -> crate::Job {
    Box::pin(async move {
        match work.await {
            Ok(reply) => Outcome::Ok(reply),
            Err(error) => Outcome::Error(protocol_error(&error)),
        }
    })
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

pub(crate) async fn session_reply(sessions: &Sessions, session: &Session) -> Result<Reply, Error> {
    Ok(Reply::Session {
        session: Box::new(current_view(sessions, session).await?),
    })
}

/// The session's view with the start files that changed since its launch, compared now.
async fn current_view(sessions: &Sessions, session: &Session) -> Result<SessionView, Error> {
    let changed = sessions.changed_start_files(session).await?;
    Ok(session_view(session, changed))
}

fn session_view(session: &Session, changed_start_files: Vec<String>) -> SessionView {
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
    }
}
