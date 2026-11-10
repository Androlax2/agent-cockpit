//! The coordinator kit (#410): an agent's `agent-sessions`, `agent-message`, `agent-start`
//! and `agent-end` (#713) from the agent socket, the owner's `session-grant`,
//! `session-revoke` and `session-grants`, the delivery of queued messages, and the
//! coordinator notes on session cards. The rules live in `cockpit-authorization` (grants,
//! start terms and cap, audit), `cockpit-approvals` (a start waiting for the owner),
//! `cockpit-messaging` (the outbox) and, for the finished label an end needs,
//! [`crate::sessions::is_finished`]; this file maps and wires.
//!
//! A message reaches its target one of two ways. An agent whose bridge collects its own
//! ([`Capability::Inbox`], Claude Code) takes it with `agent-inbox` from a hook, which hands it
//! to the model as input of its own once the agent is not working (#703). Any other is typed
//! into the target's terminal through `Sessions::submit`, the input path sessions have until
//! provider delivery (#431), and read at its next prompt.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cockpit_agent_channel::{ProcessError, ProcessIdentity};
use cockpit_approvals::{Approved, NoResult, RunRequest, Runner, StartAsk, StartSession};
use cockpit_authorization::{
    Actor, Audit, AuditEntry, AuditedAction, Basis, GrantId, Outcome, Permission, Refusal, Scope,
    SessionId as Holder, StartAllowance, StartTerms,
};
use cockpit_messaging::{
    Claim, Coalescing, Deliverer, Delivery, DeliveryState, Enqueued, Envelope, EnvelopeId, Failure,
    Redelivery,
};
use cockpit_protocol::{
    Command, CoordinatorAuditView, CoordinatorGrantView, ErrorCode, HeldMessageView,
    InboundMessageView, InboxView, ObservedSessionView, ProtocolError, Reply, Unblock, UnblockedBy,
};
use cockpit_provider_api::Capability;
use cockpit_sessions::{
    Addressed, AgentState, Ender, QuestionSource, Session, SessionId, SessionReport, SessionState,
    Sessions, StartRequest, WakeCause,
};
use cockpit_tasks::SessionWork;
use cockpit_turso::Database;
use cockpit_turso::sql::Connection;
use tokio::sync::Notify;

use crate::approvals::{Answer, Failed, Sender, internal, job, now};
use crate::routes::{Route, WorkClass};

/// The outbox producer whose envelopes are coordinator messages the delivery loop types.
const PRODUCER: &str = "coordinator";
/// The outbox producer whose envelopes are coordinator messages the target's own hook
/// collects with `agent-inbox` (#703); the delivery loop never claims them.
const INBOX_PRODUCER: &str = "coordinator-inbox";
/// How long a collected message may wait while its target's agent waits at its prompt before
/// the sender is told it is stuck: the hook polls every two seconds.
const STUCK_AFTER_SECONDS: i64 = 60;
/// How long a message's sender waits for the wake of its asleep target before it is told the
/// wake is under way: well within the CLI's 10 s request deadline.
const WAKE_ANSWER_WITHIN: Duration = Duration::from_secs(5);
/// Longest message, in characters: a line an agent reads, not a document.
const MESSAGE_MAX_CHARS: usize = 2000;
/// A message not delivered within a day is stale; the sender learns so from the card.
const MESSAGE_LIFETIME_SECONDS: i64 = 24 * 60 * 60;
/// Tries per message: a target busy with a dialog for the whole day still gets every retry.
const MESSAGE_MAX_ATTEMPTS: u32 = 10_000;
/// Deliveries one pass makes at most; the rest wait for the next pass.
const DELIVERIES_PER_PASS: usize = 32;
/// The wait before trying again a message that could not be typed, doubled while nothing is
/// delivered, up to the longest.
const FIRST_RETRY: Duration = Duration::from_secs(2);
const LONGEST_RETRY: Duration = Duration::from_secs(30);
/// Audit entries `session-grants` shows.
const AUDIT_SHOWN: u32 = 20;
/// How many times an approved start's end is recorded before it is only logged.
const FINISH_ATTEMPTS: u32 = 5;

/// What the coordinator commands and the delivery loop share.
#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) database: Arc<Database>,
    pub(crate) sessions: Arc<Sessions>,
    pub(crate) runner: Runner,
    /// Wakes the delivery loop when a message is queued.
    pub(crate) queued: Arc<Notify>,
    /// Each session's current inbox waiter.
    pub(crate) waiters: Arc<Waiters>,
    /// Starts without confirmation whose session has no row yet (#739).
    pub(crate) launching: Arc<crate::coordinator_workers::Launching>,
}

/// The current inbox waiter of each session (#703). A hook begins a waiter at each turn's
/// end; only the newest one goes on waiting, so they never pile up. Kept in memory: after the
/// daemon restarts, the first waiter that asks again becomes its session's current one.
pub(crate) struct Waiters {
    state: std::sync::Mutex<WaiterState>,
}

struct WaiterState {
    last: u64,
    current: HashMap<String, u64>,
}

impl Waiters {
    /// Ids start from the clock, so a waiter left by an earlier run of the daemon does not
    /// share its id with a new one.
    pub(crate) fn new() -> Waiters {
        let last = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_micros()).unwrap_or(u64::MAX / 2)
            });
        Waiters {
            state: std::sync::Mutex::new(WaiterState {
                last,
                current: HashMap::new(),
            }),
        }
    }

    /// A new waiter for `session`, which supersedes its earlier one.
    fn begin(&self, session: &str) -> u64 {
        let mut state = self.lock();
        state.last += 1;
        let waiter = state.last;
        state.current.insert(session.to_owned(), waiter);
        waiter
    }

    /// Whether `waiter` is still `session`'s current one.
    fn is_current(&self, session: &str, waiter: u64) -> bool {
        let mut state = self.lock();
        let current = *state.current.entry(session.to_owned()).or_insert(waiter);
        current == waiter
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WaiterState> {
        // The map holds plain ids: one a panicking holder left is still consistent.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The route for a coordinator verb from the agent socket. Agent requests run in the bulk
/// lane, like every agent request (#32), except an end: it may wait out the worker's grace
/// and kill wait, so it runs in the stop lane like the owner's (#217), never in control.
pub(crate) fn route_agent(
    command: Command,
    connector: Result<ProcessIdentity, ProcessError>,
    context: Context,
) -> Route {
    let class = if matches!(command, Command::AgentEnd { .. }) {
        WorkClass::Stop
    } else {
        WorkClass::Bulk
    };
    let work = job(async move {
        let sender = Sender::read(connector).await;
        match command {
            Command::AgentSessions { launch_token } => {
                observe(&context, &sender, launch_token).await
            }
            Command::AgentMessage {
                launch_token,
                session,
                text,
            } => message(&context, &sender, launch_token, session, text).await,
            Command::AgentInbox {
                launch_token,
                waiter,
            } => collect(&context, &sender, launch_token, waiter).await,
            Command::AgentEscalate {
                launch_token,
                session,
            } => escalate(&context, &sender, launch_token, session).await,
            Command::AgentEnd {
                launch_token,
                session,
            } => end(&context, &sender, launch_token, session).await,
            Command::AgentStart {
                launch_token,
                dir,
                agent,
                prompt,
                clone,
                model,
                effort,
            } => {
                start(
                    &context,
                    &sender,
                    launch_token,
                    &dir,
                    StartSession {
                        prompt,
                        clone,
                        agent,
                        model,
                        effort,
                    },
                )
                .await
            }
            other => unreachable!("routes.rs sends only coordinator verbs here, not {other:?}"),
        }
    });
    Route::Run { class, job: work }
}

/// The route for the owner's coordinator grants; small and the owner's: the control lane.
pub(crate) fn route_owner(command: Command, context: Context) -> Route {
    let work = job(async move {
        match command {
            Command::SessionGrant {
                session,
                permission,
                all_projects,
                without_confirmation,
                per_hour,
                max_active,
            } => {
                give(
                    &context,
                    &session,
                    &permission,
                    all_projects,
                    StartChoices {
                        without_confirmation,
                        per_hour,
                        max_active,
                    },
                )
                .await
            }
            Command::SessionRevoke {
                session,
                permission,
            } => take_back(&context, &session, &permission).await,
            Command::SessionGrants { session } => {
                let holder = holder(&session)?;
                let read = context.database.read().await?;
                grants_reply(&read, &holder).await
            }
            other => unreachable!("routes.rs sends only coordinator grants here, not {other:?}"),
        }
    });
    Route::Run {
        class: WorkClass::Control,
        job: work,
    }
}

/// `agent-sessions`: every other session not ended that the sender's `observe-sessions`
/// grant covers (its project's, or every project's, #1131), with its agent's latest report.
/// Read only: the sender's launch is bound with a write only when it is not yet.
async fn observe(context: &Context, sender: &Sender, launch_token: Option<String>) -> Answer {
    let coordinator = identified(&context.database, sender, launch_token).await?;
    let read = context.database.read().await?;
    let own_scope = scope_of(&read, &coordinator).await?;
    let decision = cockpit_authorization::evaluate(
        &read,
        &Actor::Session(coordinator.clone()),
        Permission::ObserveSessions,
        &own_scope,
    )
    .await?;
    if let Outcome::Refused(refusal) = decision.outcome {
        return Err(not_granted(&read, &coordinator, &refusal).await);
    }
    let mut reports: HashMap<String, cockpit_protocol::SessionReport> =
        crate::agent::wire_reports(&read, cockpit_sessions::latest_reports(&read).await?)
            .await?
            .into_iter()
            .map(|report| (report.session.clone(), report))
            .collect();
    let notes = notes(&read).await?;
    drop(read);
    let listed = context.sessions.list(false).await?;
    let read = context.database.read().await?;
    let mut observed = Vec::new();
    let mut covered = HashMap::from([(own_scope, true)]);
    for session in listed {
        if session.id.as_str() == coordinator.as_str() {
            continue;
        }
        let scope = cockpit_sessions::coordinator_scope(&read, &session).await?;
        let is_covered = match covered.get(&scope) {
            Some(is_covered) => *is_covered,
            None => {
                let decision = cockpit_authorization::evaluate(
                    &read,
                    &Actor::Session(coordinator.clone()),
                    Permission::ObserveSessions,
                    &scope,
                )
                .await?;
                let is_covered = matches!(decision.outcome, Outcome::Allowed(_));
                covered.insert(scope, is_covered);
                is_covered
            }
        };
        if is_covered {
            observed.push(session);
        }
    }
    drop(read);
    let sessions = observed
        .into_iter()
        .map(|session| ObservedSessionView {
            // Its own message only: what other coordinators wrote is not this one's to read.
            inbound_message: notes
                .inbound_message_of(session.id.as_str())
                .filter(|inbound| inbound.from == coordinator.as_str()),
            report: reports.remove(session.id.as_str()),
            id: session.id.to_string(),
            agent: session.provider.to_string(),
            state: crate::sessions::state_view(session.state),
            working_dir: session.working_dir.to_string_lossy().into_owned(),
            harness_trust: session.launch.and_then(|launch| launch.harness_trust),
        })
        .collect();
    Ok(Reply::AgentSessions { sessions })
}

/// `agent-message`: audits the message and queues it in one transaction; the delivery loop
/// types it at the target's prompt.
async fn message(
    context: &Context,
    sender: &Sender,
    launch_token: Option<String>,
    target: String,
    text: String,
) -> Answer {
    check_message_text(&text)?;
    let target_id = SessionId::parse(&target)?;
    let target_holder = holder(&target)?;
    // Whether the target exists, or is over, is for a sender holding the grant to learn.
    let coordinator = identified(&context.database, sender, launch_token.clone()).await?;
    if coordinator.as_str() == target {
        return Err(refused(
            ErrorCode::InvalidArgument,
            "a session does not message itself".to_owned(),
            "name another session from `torlek agent sessions`".to_owned(),
            UnblockedBy::Agent,
        ));
    }
    {
        let read = context.database.read().await?;
        // An unknown target is asked over everything: only a sender whose grant covers every
        // project learns it does not exist, and to one scoped to a project it reads as a
        // session of another project, so probing ids tells it nothing (#1131).
        let scope = match cockpit_sessions::session(&read, &target_id).await {
            Ok(target_session) => {
                cockpit_sessions::coordinator_scope(&read, &target_session).await?
            }
            Err(cockpit_sessions::Error::UnknownSession { .. }) => Scope::Owner,
            Err(error) => return Err(error.into()),
        };
        let decision = cockpit_authorization::evaluate(
            &read,
            &Actor::Session(coordinator.clone()),
            Permission::MessageSessions,
            &scope,
        )
        .await?;
        granted(&read, &coordinator, decision.outcome).await?;
    }
    let write = context.database.write().await?;
    let coordinator = sender.identify(&write, launch_token).await?;
    // Read in this transaction, so the sender learns the state the message is queued in.
    let target_session = cockpit_sessions::session(&write, &target_id).await?;
    let scope = cockpit_sessions::coordinator_scope(&write, &target_session).await?;
    let decision = cockpit_authorization::evaluate(
        &write,
        &Actor::Session(coordinator.clone()),
        Permission::MessageSessions,
        &scope,
    )
    .await?;
    let grant = granted(&write, &coordinator, decision.outcome).await?;
    refuse_if_over(&target_session)?;
    let report = cockpit_sessions::latest_report(&write, &target_holder).await?;
    // An unknown provider is not refused here: `delivery_target` below names it to the sender.
    let is_collected = context
        .sessions
        .collects_messages(&target_session)
        .is_ok_and(|collects| collects);
    let channel = if is_collected {
        Capability::Inbox
    } else {
        Capability::Submit
    };
    let waits = match context.sessions.delivery_target(&target_session, channel) {
        Err(error) => Some(error.to_string()),
        Ok(_) if is_collected => collection_wait(&target, report.as_ref()),
        Ok(_) => None,
    };
    if report.is_some_and(|report| report.question_source == Some(QuestionSource::Dialog)) {
        return Err(refused(
            ErrorCode::Conflict,
            format!("session {target} has a dialog open; a message would answer it"),
            format!("send it again once session {target}'s dialog is answered"),
            UnblockedBy::Owner,
        ));
    }
    let at = now();
    let entry = cockpit_authorization::record_audit(
        &write,
        &Audit {
            actor: coordinator.clone(),
            permission: Permission::MessageSessions,
            grant,
            revision: decision.revision,
            action: AuditedAction::Message,
            target: Some(target_holder),
            detail: Some(text.clone()),
            at,
        },
    )
    .await?;
    let enqueued = cockpit_messaging::enqueue(
        &write,
        &Envelope {
            effect_key: message_key(entry),
            producer: if is_collected {
                INBOX_PRODUCER
            } else {
                PRODUCER
            }
            .to_owned(),
            destination: target.clone(),
            payload: message_line(&coordinator, &text),
            // Typing it twice would say it twice.
            redelivery: Redelivery::Never,
            coalescing: Coalescing::Never,
            max_attempts: MESSAGE_MAX_ATTEMPTS,
            expires_at: Some(at + MESSAGE_LIFETIME_SECONDS),
        },
        at,
    )
    .await?;
    write.commit().await?;
    let waits = match enqueued {
        Enqueued::Queued { id, .. } if target_session.state == SessionState::Asleep => {
            Some(wake_for_message(context, target_id, id).await)
        }
        _ => waits,
    };
    if !is_collected {
        context.queued.notify_one();
    }
    tracing::info!(
        from = coordinator.as_str(),
        to = target.as_str(),
        audit = entry,
        is_collected,
        "a coordinator queued a message"
    );
    Ok(Reply::AgentMessageQueued {
        session: target,
        message: entry,
        waits,
        collected: is_collected,
    })
}

/// Wakes `target`, asleep, for a message whose sender the grant check let through (#1326):
/// the message is handed over once the woken agent waits at its prompt. What the sender is
/// told: the wake's outcome, or that it is still under way after [`WAKE_ANSWER_WITHIN`], so
/// a slow launch never outlasts the sender's request. A wake that fails is kept on the
/// message's card while the session sleeps.
async fn wake_for_message(context: &Context, target: SessionId, envelope: EnvelopeId) -> String {
    let waking = format!(
        "session {target} is waking from sleep; the message is handed over once its agent \
         waits at its prompt"
    );
    let still_waking = format!(
        "session {target} is waking from sleep; its card says if the wake fails, otherwise the \
         message is handed over once its agent waits at its prompt"
    );
    let database = Arc::clone(&context.database);
    let sessions = Arc::clone(&context.sessions);
    let wake = tokio::spawn(async move {
        let error = match sessions.ensure_awake(&target, WakeCause::Message).await {
            Ok(_) => {
                tracing::info!(
                    session = target.as_str(),
                    "a message woke its asleep target"
                );
                return None;
            }
            Err(error) => error,
        };
        let reason = format!(
            "session {target} is asleep, and waking it for the message failed: {error}; the \
             owner wakes it with `torlek session wake {target}`"
        );
        tracing::warn!(session = target.as_str(), %error, "a message could not wake its target");
        if let Err(Failed(failed)) = record_note(&database, envelope, &reason).await {
            tracing::error!(
                session = target.as_str(),
                error = failed.message,
                "recording why a message waits failed"
            );
        }
        Some(reason)
    });
    match tokio::time::timeout(WAKE_ANSWER_WITHIN, wake).await {
        Ok(Ok(None)) => waking,
        Ok(Ok(Some(failure))) => failure,
        Ok(Err(error)) => {
            tracing::error!(%error, "waking a message's target stopped");
            still_waking
        }
        Err(_) => still_waking,
    }
}

async fn record_note(database: &Database, envelope: EnvelopeId, note: &str) -> Result<(), Failed> {
    let write = database.write().await?;
    cockpit_messaging::record_note(&write, envelope, note).await?;
    write.commit().await?;
    Ok(())
}

/// `agent-inbox` (#703): hands the sender's agent its oldest message still queued, recorded
/// as received in the same transaction, unless its agent works or has a dialog open: a
/// message waits for the turn's end. A message whose sender lost `message-sessions` fails and
/// the next one is tried. Hands over nothing to a waiter a newer one superseded.
async fn collect(
    context: &Context,
    sender: &Sender,
    launch_token: Option<String>,
    waiter: Option<u64>,
) -> Answer {
    let session = identified(&context.database, sender, launch_token).await?;
    let waiter = match waiter {
        None => context.waiters.begin(session.as_str()),
        Some(waiter) if context.waiters.is_current(session.as_str(), waiter) => waiter,
        Some(waiter) => {
            return Ok(Reply::AgentInbox {
                waiter,
                inbox: InboxView::Superseded,
            });
        }
    };
    let empty = Reply::AgentInbox {
        waiter,
        inbox: InboxView::Empty,
    };
    // Each idle session's waiter asks every 2 s: the single writer is taken only when a
    // message waits and the agent can take it, which is checked again under the writer.
    {
        let read = context.database.read().await?;
        if is_busy(&read, &session).await?
            || !cockpit_messaging::has_queued_for(&read, INBOX_PRODUCER, session.as_str()).await?
        {
            return Ok(empty);
        }
    }
    let write = context.database.write().await?;
    if is_busy(&write, &session).await? {
        return Ok(empty);
    }
    let at = now();
    while let Some(claim) =
        cockpit_messaging::claim_next_for(&write, INBOX_PRODUCER, session.as_str(), at).await?
    {
        let (delivery, from) = match authorship(&write, &claim).await.map_err(|reason| {
            tracing::error!(%reason, "reading a collected message's sender failed");
            internal(&reason)
        })? {
            Authorship::Rejected(reason) => (Delivery::Failed(Failure::Rejected { reason }), None),
            Authorship::Granted(from) => (
                Delivery::Received {
                    receipt: format!(
                        "handed to session {}'s agent by its hook, from session {}",
                        session.as_str(),
                        from.as_str()
                    ),
                },
                Some(from),
            ),
        };
        let state =
            cockpit_messaging::record_delivery(&write, claim.id, claim.attempt, &delivery, at)
                .await?;
        let Some(from) = from.filter(|_| state == DeliveryState::Received) else {
            continue;
        };
        // A collected message starts no prompt, so nothing else clears the question the
        // coordinator that started the session answers with it (#710); a typed one does.
        cockpit_sessions::answer_question(&write, &session, &from).await?;
        let message =
            audit_entry_of(&claim.effect_key).ok_or_else(|| internal(&claim.effect_key))?;
        write.commit().await?;
        tracing::info!(
            to = session.as_str(),
            audit = message,
            "a session's hook collected a coordinator message"
        );
        return Ok(Reply::AgentInbox {
            waiter,
            inbox: InboxView::Message {
                message,
                line: claim.payload,
            },
        });
    }
    // Expired messages are recorded.
    write.commit().await?;
    Ok(empty)
}

/// Whether `session`'s agent cannot take a message now: it works, or a dialog waits for the
/// owner.
async fn is_busy(connection: &Connection, session: &Holder) -> Result<bool, Failed> {
    let report = cockpit_sessions::latest_report(connection, session).await?;
    Ok(report.is_some_and(|report| {
        report.state == Some(AgentState::Working)
            || report.question_source == Some(QuestionSource::Dialog)
    }))
}

/// `agent-escalate` (#710): the coordinator that started `target` hands the question its
/// agent asked to the owner, before its time with the coordinator is over. Needs no grant:
/// starting the session is what made the question the coordinator's.
async fn escalate(
    context: &Context,
    sender: &Sender,
    launch_token: Option<String>,
    target: String,
) -> Answer {
    let target_id = SessionId::parse(&target)?;
    let target_holder = holder(&target)?;
    let write = context.database.write().await?;
    let coordinator = sender.identify(&write, launch_token).await?;
    let target_session = cockpit_sessions::session(&write, &target_id).await?;
    if target_session.started_by.as_ref().map(SessionId::as_str) != Some(coordinator.as_str()) {
        return Err(refused(
            ErrorCode::NotAuthorized,
            format!(
                "session {} did not start session {target}; its question is not the sender's to escalate",
                coordinator.as_str()
            ),
            "escalate only a session you started with `torlek agent start`".to_owned(),
            UnblockedBy::Agent,
        ));
    }
    if !cockpit_sessions::escalate_question(&write, &target_holder, now()).await? {
        return Err(refused(
            ErrorCode::Conflict,
            format!("session {target} has no question of its own open"),
            format!(
                "nothing waits; `torlek agent sessions` shows session {target}'s question once it asks"
            ),
            UnblockedBy::Agent,
        ));
    }
    write.commit().await?;
    tracing::info!(
        from = coordinator.as_str(),
        session = target.as_str(),
        "a coordinator escalated a question to the owner"
    );
    Ok(Reply::AgentEscalated { session: target })
}

/// `agent-end` (#713): ends `target` for the coordinator that started it, once the task list
/// says its work is done (the finished label, #157: doing no item, finished one since its
/// latest working launch). Needs `end-sessions`; the audit names both sessions and the item.
/// Any other session, finished or not, and every other lifecycle change stay the owner's:
/// `routes.rs` never sends them here.
async fn end(
    context: &Context,
    sender: &Sender,
    launch_token: Option<String>,
    target: String,
) -> Answer {
    let target_id = SessionId::parse(&target)?;
    let target_holder = holder(&target)?;
    let coordinator = identified(&context.database, sender, launch_token).await?;
    if coordinator.as_str() == target {
        return Err(refused(
            ErrorCode::InvalidArgument,
            "a session does not end itself".to_owned(),
            "name a session you started, from `torlek agent sessions`".to_owned(),
            UnblockedBy::Agent,
        ));
    }
    // Decided under the target's lock, right before its agent is stopped: a grant revoked, or
    // an item the worker took, between a check and the end would otherwise be missed.
    let (ended, (grant, finished_item)) = context
        .sessions
        .end_by_when(
            &target_id,
            &Ender::Coordinator(SessionId::parse(coordinator.as_str())?),
            async |target_session: &Session| {
                // Checked first: no grant, over any scope, lets it end a session it did not
                // start, so a scope refusal must not suggest one would.
                if target_session.started_by.as_ref().map(SessionId::as_str)
                    != Some(coordinator.as_str())
                {
                    return Err(refused(
                        ErrorCode::NotAuthorized,
                        format!(
                            "session {} did not start session {target}; only the owner ends it",
                            coordinator.as_str()
                        ),
                        "end only a session you started with `torlek agent start`; ask the owner for any other".to_owned(),
                        UnblockedBy::Agent,
                    ));
                }
                let read = context.database.read().await?;
                let scope = cockpit_sessions::coordinator_scope(&read, target_session).await?;
                let decision = cockpit_authorization::evaluate(
                    &read,
                    &Actor::Session(coordinator.clone()),
                    Permission::EndSessions,
                    &scope,
                )
                .await?;
                let grant = granted(&read, &coordinator, decision.outcome).await?;
                refuse_if_over(target_session)?;
                let work = cockpit_tasks::work_of(&read, &target_holder).await?;
                if !crate::sessions::is_finished_with(target_session, &work) {
                    return Err(unfinished(&target, target_session, &work));
                }
                let finished_item = cockpit_tasks::latest_finished_item(&read, &target_holder)
                    .await?
                    .ok_or_else(|| {
                        internal(&format!(
                            "session {target} is labelled finished, but the task list names no item it finished"
                        ))
                    })?;
                Ok::<_, Failed>((grant, finished_item))
            },
        )
        .await?;
    tracing::info!(
        from = coordinator.as_str(),
        session = %ended.id,
        "a coordinator ended a session it started"
    );
    // The session is over: from here an error must not read as an end that did not happen.
    if let Err(Failed(error)) = record_ended(
        &context.database,
        &coordinator,
        grant,
        &target_holder,
        format!("item {}", finished_item.0),
    )
    .await
    {
        tracing::error!(session = %ended.id, ?error, "an end's `ended` entry was not audited");
    }
    Ok(Reply::AgentEnded { session: target })
}

/// Why `target` is not finished, in the words of the rule that fails: its state, the items
/// it is doing, or the item it finished since its launch that the task list does not record.
fn unfinished(target: &str, session: &Session, work: &SessionWork) -> Failed {
    let (why, then) = match session.state {
        SessionState::Launching => (
            "it is still launching".to_owned(),
            format!("end session {target} once it runs and finishes its item"),
        ),
        SessionState::Stopping { .. } => (
            "it is already stopping".to_owned(),
            format!("`torlek agent sessions` shows session {target} once its stop is over"),
        ),
        _ if work.doing > 0 => (
            format!("it is doing {} item(s)", work.doing),
            format!("end session {target} once `torlek todo done` marks its item done"),
        ),
        _ => (
            "the task list records no item it finished since its launch".to_owned(),
            format!("end session {target} once `torlek todo done` marks its item done"),
        ),
    };
    refused(
        ErrorCode::Conflict,
        format!("session {target} has not finished its work: {why}"),
        then,
        UnblockedBy::Agent,
    )
}

/// Audits the end of `target` by `coordinator`, with the item it finished as the detail.
async fn record_ended(
    database: &Database,
    coordinator: &Holder,
    grant: GrantId,
    target: &Holder,
    detail: String,
) -> Result<(), Failed> {
    let write = database.write().await?;
    let revision = cockpit_authorization::current_revision(&write).await?;
    cockpit_authorization::record_audit(
        &write,
        &Audit {
            actor: coordinator.clone(),
            permission: Permission::EndSessions,
            grant,
            revision,
            action: AuditedAction::Ended,
            target: Some(target.clone()),
            detail: Some(detail),
            at: now(),
        },
    )
    .await?;
    write.commit().await?;
    Ok(())
}

/// `agent-start`: below the sessions at once and within the hourly cap, either files the
/// start in the `ask run` queue or, when the grant says no confirmation, starts it now. Each
/// counts against the hourly cap once recorded, whether it then starts or not.
async fn start(
    context: &Context,
    sender: &Sender,
    launch_token: Option<String>,
    dir: &str,
    start: StartSession,
) -> Answer {
    let dir = cockpit_approvals::resolve_cwd(dir)?;
    cockpit_approvals::validate_start(&dir, &start)?;
    // Refused before the owner is asked: an approval must not start a launch that fails.
    context
        .sessions
        .check_launch_choices(
            start.agent.as_deref(),
            start.model.as_deref(),
            start.effort.as_deref(),
            None,
        )
        .await?;
    let write = context.database.write().await?;
    let coordinator = sender.identify(&write, launch_token).await?;
    let at = now();
    let active = crate::coordinator_workers::active_workers(&write, &coordinator, at).await?
        + context.launching.count(&coordinator);
    let project = crate::approvals::project_at(&write, &dir).await?;
    // A directory in no project is covered by a grant over everything only (#1131).
    let scope = match &project {
        Some(project) => project_scope(&project.name)?,
        None => Scope::Owner,
    };
    let (grant, revision, needs_confirmation) =
        match cockpit_authorization::start_allowance(&write, &coordinator, &scope, at, active)
            .await?
        {
            StartAllowance::Allowed {
                grant,
                revision,
                needs_confirmation,
            } => (grant, revision, needs_confirmation),
            StartAllowance::Capped { per_hour, retry_at } => {
                return Err(refused(
                    ErrorCode::Refused,
                    format!(
                        "session {} has made its {per_hour} starts of the past hour",
                        coordinator.as_str()
                    ),
                    format!("start again at {retry_at} (seconds since 1970)"),
                    UnblockedBy::Agent,
                ));
            }
            StartAllowance::AtCapacity { max_active, active } => {
                return Err(refused(
                    ErrorCode::Refused,
                    format!(
                        "session {} is at its {max_active} sessions at once: {active} it \
                         started run or wait for the owner",
                        coordinator.as_str()
                    ),
                    "a place frees when one of them ends (its agent exits, or the owner ends \
                     it with `torlek session end ID`) or the owner declines its start request"
                        .to_owned(),
                    UnblockedBy::Owner,
                ));
            }
            StartAllowance::Refused(refusal) => {
                return Err(not_granted(&write, &coordinator, &refusal).await);
            }
        };
    let mut audit = Audit {
        actor: coordinator.clone(),
        permission: Permission::StartSessions,
        grant,
        revision,
        action: AuditedAction::Start,
        target: None,
        detail: None,
        at,
    };
    if needs_confirmation {
        let ask = StartAsk {
            dir,
            start,
            reason: format!(
                "session {} holds start-sessions and asks to start one",
                coordinator.as_str()
            ),
            project,
        };
        let run = cockpit_approvals::ask_start(&write, &coordinator, ask, grant, at).await?;
        audit.detail = Some(format!("approval request {}", run.id.0));
        cockpit_authorization::record_audit(&write, &audit).await?;
        write.commit().await?;
        tracing::info!(
            request = run.id.0,
            session = coordinator.as_str(),
            "a coordinator asked to start a session"
        );
        return Ok(crate::approvals::run_reply(&run, &context.runner));
    }
    // An owner auditing the session must tell a start nobody confirmed from an owner's yes.
    audit.detail = Some("without confirmation".to_owned());
    cockpit_authorization::record_audit(&write, &audit).await?;
    let launching = context.launching.begin(&coordinator);
    write.commit().await?;
    let started = context
        .sessions
        .start(start_request(dir, start, &coordinator)?)
        .await?;
    drop(launching);
    tracing::info!(
        from = coordinator.as_str(),
        session = %started.id,
        "a coordinator started a session without confirmation"
    );
    // The session runs: from here an error must not read as a start that did not happen,
    // or the agent would start another.
    if let Err(Failed(error)) =
        record_started(&context.database, &coordinator, grant, &started).await
    {
        tracing::error!(session = %started.id, ?error, "a start's `started` entry was not audited");
    }
    let view = async {
        let inputs = crate::sessions::card_inputs(&context.sessions).await?;
        Ok::<_, Failed>(crate::sessions::current_view(&context.sessions, &started, &inputs).await?)
    };
    match view.await {
        Ok(view) => Ok(Reply::AgentStarted {
            session: Box::new(view),
        }),
        Err(Failed(error)) => Err(Failed(ProtocolError::new(
            ErrorCode::Internal,
            format!(
                "session {} started, but its view could not be read: {}",
                started.id, error.message
            ),
        ))),
    }
}

/// Carries out a start request the owner approved: starts the session, then finishes the
/// request with it and audits it; a start that fails leaves the request not run.
pub(crate) async fn start_approved(
    database: Arc<Database>,
    sessions: Arc<Sessions>,
    approved: Approved,
    asker: Holder,
    asked_under: Option<i64>,
) {
    let request = approved.id.0;
    let start = approved
        .start
        .clone()
        .expect("only a start request is carried out as a start");
    let start = match start_request(approved.cwd.clone(), start, &asker) {
        Ok(start) => start,
        Err(Failed(error)) => {
            tracing::warn!(
                request,
                ?error,
                "an approved start names no valid asking session"
            );
            let not_started = Err(NoResult::NotStarted(error.message));
            crate::approvals::record_end_or_log(&database, &approved, &not_started).await;
            return;
        }
    };
    match sessions.start(start).await {
        Ok(started) => {
            tracing::info!(request, session = %started.id, "an approved start began");
            let grant = finish_approved(&database, &approved, asked_under, &started).await;
            if let Some(grant) = grant
                && let Err(Failed(error)) = record_started(&database, &asker, grant, &started).await
            {
                tracing::error!(
                    request,
                    ?error,
                    "an approved start's `started` entry was not audited"
                );
            }
        }
        Err(error) => {
            tracing::warn!(request, %error, "an approved start did not begin");
            let not_started = Err(NoResult::NotStarted(error.to_string()));
            crate::approvals::record_end_or_log(&database, &approved, &not_started).await;
        }
    }
}

/// Records the approved request as finished with the session it started, trying again
/// while the database refuses: a request left `approved` would read as never run after a
/// restart, while its session runs. Returns the grant it was asked under.
async fn finish_approved(
    database: &Database,
    approved: &Approved,
    asked_under: Option<i64>,
    started: &Session,
) -> Option<GrantId> {
    let request = approved.id.0;
    let mut wait = FIRST_RETRY;
    for attempt in 1..=FINISH_ATTEMPTS {
        match try_finish_approved(database, approved, asked_under, started).await {
            Ok(grant) => return Some(grant),
            Err(Failed(error)) if attempt < FINISH_ATTEMPTS => {
                tracing::warn!(
                    request,
                    attempt,
                    ?error,
                    "an approved start's end was not recorded; trying again"
                );
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(LONGEST_RETRY);
            }
            Err(Failed(error)) => {
                tracing::error!(
                    request,
                    session = %started.id,
                    ?error,
                    "an approved start's end was not recorded; its session runs"
                );
            }
        }
    }
    None
}

async fn try_finish_approved(
    database: &Database,
    approved: &Approved,
    asked_under: Option<i64>,
    started: &Session,
) -> Result<GrantId, Failed> {
    let grant = match asked_under {
        Some(id) => {
            let read = database.read().await?;
            cockpit_authorization::stored_grant(&read, id).await?
        }
        None => None,
    }
    .ok_or_else(|| internal(&"an approved start request names no stored start-sessions grant"))?;
    let write = database.write().await?;
    cockpit_approvals::finish_start(&write, approved, started.id.as_str(), now()).await?;
    write.commit().await?;
    Ok(grant.id)
}

/// The start a coordinator asked for, recorded as started by it (#710).
fn start_request(
    dir: PathBuf,
    start: StartSession,
    coordinator: &Holder,
) -> Result<StartRequest, Failed> {
    Ok(StartRequest {
        dir,
        provider: start.agent,
        first_message: start.prompt,
        clone_branch: start.clone,
        model: start.model,
        effort: start.effort,
        // A coordinator's start chooses no compaction threshold yet (#783).
        compaction_threshold: None,
        started_by: Some(SessionId::parse(coordinator.as_str())?),
    })
}

/// Audits the owner's decision on a start request beside its ask (#657), in the decision's
/// transaction: the audit then tells an owner's yes or no from a start nobody confirmed.
/// A command request has no coordinator grant to audit under. A start whose grant is not
/// stored is decided all the same, unaudited and logged: the audit must not keep the owner
/// from declining.
pub(crate) async fn record_decision(
    write: &cockpit_turso::WriteTransaction<'_>,
    run: &RunRequest,
) -> Result<(), Failed> {
    let (Some(_), Some(decision)) = (&run.start, run.decision) else {
        return Ok(());
    };
    let grant = match run.asking_grant() {
        Some(id) => cockpit_authorization::stored_grant(write, id).await?,
        None => None,
    };
    let Some(grant) = grant else {
        tracing::warn!(
            request = run.id.0,
            "a decided start request names no stored start-sessions grant; the decision is not audited"
        );
        return Ok(());
    };
    let detail = match decision.through {
        Some(through) => format!(
            "approval request {}, by the owner through the {}",
            run.id.0,
            through.label()
        ),
        None => format!("approval request {}, by the owner", run.id.0),
    };
    cockpit_authorization::record_audit(
        write,
        &Audit {
            actor: run.session.clone(),
            permission: Permission::StartSessions,
            grant: grant.id,
            revision: cockpit_authorization::current_revision(write).await?,
            action: if decision.approved {
                AuditedAction::Approved
            } else {
                AuditedAction::Declined
            },
            target: None,
            detail: Some(detail),
            at: decision.at,
        },
    )
    .await?;
    Ok(())
}

/// Audits the session a start made, with both sessions named.
async fn record_started(
    database: &Database,
    coordinator: &Holder,
    grant: GrantId,
    started: &Session,
) -> Result<(), Failed> {
    let write = database.write().await?;
    let revision = cockpit_authorization::current_revision(&write).await?;
    cockpit_authorization::record_audit(
        &write,
        &Audit {
            actor: coordinator.clone(),
            permission: Permission::StartSessions,
            grant,
            revision,
            action: AuditedAction::Started,
            target: Some(holder(started.id.as_str())?),
            detail: None,
            at: now(),
        },
    )
    .await?;
    write.commit().await?;
    Ok(())
}

/// `session-grant`'s terms of `start-sessions`.
struct StartChoices {
    without_confirmation: bool,
    per_hour: Option<u32>,
    max_active: Option<u32>,
}

/// `session-grant`: gives `session` one coordinator grant over its own project, or over every
/// project with `all_projects` (#1131). Giving one it holds changes nothing; giving it over
/// another scope, or `start-sessions` with other terms, replaces it.
async fn give(
    context: &Context,
    session: &str,
    permission: &str,
    all_projects: bool,
    choices: StartChoices,
) -> Answer {
    let StartChoices {
        without_confirmation,
        per_hour,
        max_active,
    } = choices;
    let permission = coordinator_permission(permission)?;
    let has_terms = without_confirmation || per_hour.is_some() || max_active.is_some();
    if has_terms && permission != Permission::StartSessions {
        return Err(invalid_argument(
            "--without-confirmation, --per-hour and --max-active are terms of start-sessions only",
        ));
    }
    if without_confirmation && per_hour.is_none() {
        return Err(invalid_argument(
            "--without-confirmation needs --per-hour: starts nobody confirms have an explicit cap",
        ));
    }
    let id = SessionId::parse(session)?;
    let granted_session = context.sessions.get(&id).await?;
    refuse_if_over(&granted_session)?;
    let holder = holder(session)?;
    let write = context.database.write().await?;
    let scope = if all_projects {
        Scope::Owner
    } else {
        cockpit_sessions::coordinator_scope(&write, &granted_session).await?
    };
    let given = if permission == Permission::StartSessions {
        let terms = StartTerms {
            per_hour: per_hour.unwrap_or(StartTerms::DEFAULT.per_hour),
            max_active: max_active.unwrap_or(StartTerms::DEFAULT.max_active),
            needs_confirmation: !without_confirmation,
        };
        cockpit_authorization::grant_start_sessions(&write, &holder, &scope, terms).await?
    } else {
        cockpit_authorization::grant_coordinator(&write, &holder, permission, &scope).await?
    };
    write.commit().await?;
    tracing::info!(
        session,
        %permission,
        scope = %given.scope,
        grant = given.id.value(),
        "the owner gave a coordinator grant"
    );
    let read = context.database.read().await?;
    grants_reply(&read, &holder).await
}

/// `session-revoke`: takes back `session`'s coordinator grant, whatever its scope; a start it
/// waits on leaves the queue with it.
async fn take_back(context: &Context, session: &str, permission: &str) -> Answer {
    let permission = coordinator_permission(permission)?;
    let holder = holder(session)?;
    let write = context.database.write().await?;
    let revoked = cockpit_authorization::revoke_permission(&write, &holder, permission).await?;
    if revoked.is_empty() {
        return Err(Failed(ProtocolError::new(
            ErrorCode::NotFound,
            format!("session {session} holds no {permission} grant"),
        )));
    }
    write.commit().await?;
    tracing::info!(session, %permission, "the owner revoked a coordinator grant");
    let read = context.database.read().await?;
    grants_reply(&read, &holder).await
}

async fn grants_reply(connection: &Connection, holder: &Holder) -> Answer {
    let grants = cockpit_authorization::active_grants(connection, holder)
        .await?
        .into_iter()
        .filter(|grant| grant.permission.is_coordinator())
        .map(|grant| CoordinatorGrantView {
            permission: grant.permission.as_str().to_owned(),
            grant: grant.id.value(),
            scope: grant.scope.to_string(),
            per_hour: grant.start_terms.map(|terms| terms.per_hour),
            max_active: grant.start_terms.map(|terms| terms.max_active),
            needs_confirmation: grant.start_terms.map(|terms| terms.needs_confirmation),
        })
        .collect();
    let audit = cockpit_authorization::audit_of(connection, holder, AUDIT_SHOWN)
        .await?
        .into_iter()
        .map(audit_view)
        .collect();
    Ok(Reply::CoordinatorGrants {
        session: holder.as_str().to_owned(),
        grants,
        audit,
    })
}

fn audit_view(entry: AuditEntry) -> CoordinatorAuditView {
    let audit = entry.audit;
    CoordinatorAuditView {
        id: entry.id,
        actor: audit.actor.as_str().to_owned(),
        permission: audit.permission.as_str().to_owned(),
        action: audit.action.as_str().to_owned(),
        target: audit.target.map(|target| target.as_str().to_owned()),
        detail: audit.detail,
        at: audit.at,
    }
}

/// Delivers queued messages for as long as the daemon runs: at once when one is queued, then
/// again after a growing wait while some could not be typed yet. The first pass delivers
/// what a previous run of the daemon left queued.
pub(crate) async fn deliver_forever(context: Context) {
    let mut retry = None;
    loop {
        retry = match deliver(&context).await {
            Ok(Pass::Settled) => None,
            Ok(Pass::Full) => continue,
            Ok(Pass::Delivered) => Some(FIRST_RETRY),
            Ok(Pass::Waiting) => Some(longer(retry)),
            Err(error) => {
                tracing::error!(%error, "delivering coordinator messages failed");
                Some(longer(retry))
            }
        };
        match retry {
            None => context.queued.notified().await,
            Some(wait) => {
                tokio::select! {
                    () = context.queued.notified() => {}
                    () = tokio::time::sleep(wait) => {}
                }
            }
        }
    }
}

fn longer(retry: Option<Duration>) -> Duration {
    retry.map_or(FIRST_RETRY, |wait| (wait * 2).min(LONGEST_RETRY))
}

/// What a delivery pass left.
enum Pass {
    /// Nothing is queued.
    Settled,
    /// A full pass, each message delivered: more are due now.
    Full,
    /// Some were delivered and some still wait.
    Delivered,
    /// Some still wait, and none could be delivered.
    Waiting,
}

async fn deliver(context: &Context) -> Result<Pass, cockpit_messaging::Error> {
    let delivered = cockpit_messaging::deliver_due(
        &context.database,
        PRODUCER,
        &Typist {
            database: &context.database,
            sessions: &context.sessions,
        },
        now(),
        DELIVERIES_PER_PASS,
    )
    .await?;
    // Every message is due once queued, and a retry is due at once: a pass that claims none
    // finds none queued, and a full pass may have left more.
    let is_full = delivered.len() == DELIVERIES_PER_PASS;
    let is_waiting = delivered
        .iter()
        .any(|(_, state)| *state == DeliveryState::Queued);
    if !is_waiting {
        return Ok(if is_full { Pass::Full } else { Pass::Settled });
    }
    let has_delivered = delivered
        .iter()
        .any(|(_, state)| *state == DeliveryState::Received);
    Ok(if has_delivered {
        Pass::Delivered
    } else {
        Pass::Waiting
    })
}

/// Types a message line to its target's agent, under the grant its sender still holds.
struct Typist<'context> {
    database: &'context Database,
    sessions: &'context Sessions,
}

impl Deliverer for Typist<'_> {
    async fn deliver(&self, claim: &Claim) -> Delivery {
        let rejected = |reason: String| Delivery::Failed(Failure::Rejected { reason });
        let sender = match self.check(claim).await {
            Ok(sender) => sender,
            Err(delivery) => return delivery,
        };
        let target = match SessionId::parse(&claim.destination) {
            Ok(target) => target,
            Err(error) => return rejected(error.to_string()),
        };
        match self.sessions.get(&target).await {
            Ok(session) if is_over(&session) => {
                return rejected(format!("session {target} ended before delivery"));
            }
            Ok(_) => {}
            Err(error @ cockpit_sessions::Error::UnknownSession { .. }) => {
                return rejected(error.to_string());
            }
            Err(error) => return not_sent(error.to_string()),
        }
        match self
            .sessions
            .submit(&target, &claim.payload, Addressed::Collaborator)
            .await
        {
            Ok(()) => Delivery::Received {
                receipt: format!(
                    "delivered to session {target} from session {}",
                    sender.as_str()
                ),
            },
            Err(
                error @ (cockpit_sessions::Error::TypingUnconfirmed { .. }
                | cockpit_sessions::Error::DeliveryUnconfirmed { .. }),
            ) => Delivery::Failed(Failure::Unacknowledged {
                reason: error.to_string(),
                retry_at: now(),
            }),
            Err(error @ cockpit_sessions::Error::UnknownSession { .. }) => {
                rejected(error.to_string())
            }
            // Nothing was sent: a dialog was open, the agent was between launches, tmux
            // did not answer. The loop tries again.
            Err(error) => not_sent(error.to_string()),
        }
    }
}

impl Typist<'_> {
    /// The message's sender, once nothing sent before it to the same session still waits
    /// (a session reads its messages in the order they were sent) and the sender still
    /// holds `message-sessions` over it (a revoked grant, or a sender that ended, stops the
    /// messages it queued).
    async fn check(&self, claim: &Claim) -> Result<Holder, Delivery> {
        let read = self
            .database
            .read()
            .await
            .map_err(|error| not_sent(error.to_string()))?;
        if cockpit_messaging::has_earlier_queued(&read, claim)
            .await
            .map_err(|error| not_sent(error.to_string()))?
        {
            return Err(not_sent(format!(
                "an earlier message to session {} waits",
                claim.destination
            )));
        }
        match authorship(&read, claim).await.map_err(not_sent)? {
            Authorship::Granted(sender) => Ok(sender),
            Authorship::Rejected(reason) => Err(Delivery::Failed(Failure::Rejected { reason })),
        }
    }
}

/// Who sent a claimed coordinator message, as its audit entry says, and whether it may still
/// reach its destination.
enum Authorship {
    /// The sender still holds `message-sessions` over the destination.
    Granted(Holder),
    /// Never delivered: the grant is gone, or the message names no audit entry.
    Rejected(String),
}

/// The [`Authorship`] of `claim`; `Err` when it could not be read, so nothing is decided.
async fn authorship(connection: &Connection, claim: &Claim) -> Result<Authorship, String> {
    let Some(entry) = audit_entry_of(&claim.effect_key) else {
        return Ok(Authorship::Rejected(format!(
            "{} names no audited message",
            claim.effect_key
        )));
    };
    let target = match SessionId::parse(&claim.destination) {
        Ok(target) => target,
        Err(error) => return Ok(Authorship::Rejected(error.to_string())),
    };
    let Some(entry) = cockpit_authorization::audit_entry(connection, entry)
        .await
        .map_err(|error| error.to_string())?
    else {
        tracing::error!(entry, "a queued coordinator message has no audit entry");
        return Ok(Authorship::Rejected(format!(
            "audit entry {entry} is missing"
        )));
    };
    let sender = entry.audit.actor;
    let target_session = match cockpit_sessions::session(connection, &target).await {
        Ok(session) => session,
        Err(error @ cockpit_sessions::Error::UnknownSession { .. }) => {
            return Ok(Authorship::Rejected(error.to_string()));
        }
        Err(error) => return Err(error.to_string()),
    };
    let scope = cockpit_sessions::coordinator_scope(connection, &target_session)
        .await
        .map_err(|error| error.to_string())?;
    let decision = cockpit_authorization::evaluate(
        connection,
        &Actor::Session(sender.clone()),
        Permission::MessageSessions,
        &scope,
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(match decision.outcome {
        Outcome::Allowed(_) => Authorship::Granted(sender),
        Outcome::Refused(_) => Authorship::Rejected(format!(
            "session {} no longer holds message-sessions over {scope}",
            sender.as_str()
        )),
    })
}

/// The audit entry a coordinator message's outbox key names.
fn audit_entry_of(effect_key: &str) -> Option<i64> {
    effect_key
        .strip_prefix("coordinator-message:")
        .and_then(|id| id.parse::<i64>().ok())
}

/// Nothing was typed; the loop tries again.
fn not_sent(reason: String) -> Delivery {
    Delivery::Failed(Failure::NotSent {
        reason,
        retry_at: now(),
    })
}

/// What session cards show of the coordinator kit, read once per answer.
#[derive(Default)]
pub(crate) struct Notes {
    grants: HashMap<String, Vec<String>>,
    inbound: HashMap<String, InboundMessageView>,
}

impl Notes {
    /// The coordinator grants `session` holds, by name.
    pub(crate) fn grants_of(&self, session: &str) -> Vec<String> {
        self.grants.get(session).cloned().unwrap_or_default()
    }

    /// The latest message a coordinator queued to `session`.
    pub(crate) fn inbound_message_of(&self, session: &str) -> Option<InboundMessageView> {
        self.inbound.get(session).cloned()
    }
}

pub(crate) async fn notes(connection: &Connection) -> Result<Notes, Failed> {
    let mut notes = Notes::default();
    // Read only when a collected message still waits: most listings have none.
    let mut reports: Option<HashMap<String, SessionReport>> = None;
    let at = now();
    for grant in cockpit_authorization::active_coordinator_grants(connection).await? {
        notes
            .grants
            .entry(grant.holder.as_str().to_owned())
            .or_default()
            .push(grant.permission.as_str().to_owned());
    }
    for entry in
        cockpit_authorization::latest_audits_by_target(connection, AuditedAction::Message).await?
    {
        // A card note never stops session control: an inconsistent entry is logged and left
        // off the card.
        let key = message_key(entry.id);
        let Some(envelope) = cockpit_messaging::envelope_by_key(connection, &key).await? else {
            tracing::error!(key, "a coordinator message was audited but never queued");
            continue;
        };
        let audit = entry.audit;
        let (Some(target), Some(text)) = (audit.target, audit.detail) else {
            tracing::error!(key, "a coordinator message's audit names no target or text");
            continue;
        };
        let is_collected = envelope.producer == INBOX_PRODUCER;
        let is_collected_waiting = is_collected && envelope.state == DeliveryState::Queued;
        let note = if is_collected_waiting && is_asleep(connection, target.as_str()).await {
            // Its hook collects nothing while its agent sleeps; a wake that failed says why.
            Some(envelope.note.as_deref().map_or_else(
                || format!("held: session {} is asleep", target.as_str()),
                cockpit_sessions::visible_line,
            ))
        } else if is_collected_waiting {
            let reports = match reports {
                Some(ref reports) => reports,
                None => reports.insert(
                    cockpit_sessions::latest_reports(connection)
                        .await?
                        .into_iter()
                        .map(|report| (report.session.as_str().to_owned(), report))
                        .collect(),
                ),
            };
            Some(collection_note(
                target.as_str(),
                reports.get(target.as_str()),
                audit.at,
                at,
            ))
        } else {
            // A stored note says why the last attempt failed: stale once the message is in
            // flight or delivered.
            (!matches!(
                envelope.state,
                DeliveryState::Submitting | DeliveryState::Received
            ))
            .then_some(envelope.note)
            .flatten()
            .map(|note| cockpit_sessions::visible_line(&note))
        };
        let overtaken = envelope
            .overtaken_at
            .filter(|_| envelope.state == DeliveryState::Queued)
            .map(|_| {
                format!(
                    "the owner's message to session {} was delivered while it waited",
                    target.as_str()
                )
            });
        let note = match (note, overtaken) {
            (Some(note), Some(overtaken)) => Some(format!("{note}; {overtaken}")),
            (note, overtaken) => note.or(overtaken),
        };
        notes.inbound.insert(
            target.as_str().to_owned(),
            InboundMessageView {
                from: audit.actor.as_str().to_owned(),
                text,
                delivery: envelope.state.as_str().to_owned(),
                sent_at: audit.at,
                note,
            },
        );
    }
    Ok(notes)
}

/// Whether `session` sleeps; a session that cannot be read is logged and taken as awake, as
/// a card note never stops session control.
async fn is_asleep(connection: &Connection, session: &str) -> bool {
    let state = match SessionId::parse(session) {
        Ok(id) => cockpit_sessions::session(connection, &id)
            .await
            .map(|session| session.state),
        Err(error) => Err(error),
    };
    match state {
        Ok(state) => state == SessionState::Asleep,
        Err(error) => {
            tracing::error!(session, %error, "reading a messaged session's state failed");
            false
        }
    }
}

/// A coordinator message to a session still waiting, and its outbox envelope.
pub(crate) struct Held {
    envelope: EnvelopeId,
    pub(crate) view: HeldMessageView,
}

/// The coordinator messages to `session` its hook holds for the end of its agent's turn,
/// oldest first: those an owner's message sent now overtakes (#897). None while the agent can
/// take them: its hook collects them within seconds. A typed message is left out too: it is
/// typed as soon as its session takes input, so it does not wait for the turn's end.
pub(crate) async fn held_for(connection: &Connection, session: &str) -> Result<Vec<Held>, Failed> {
    let target = holder(session)?;
    if !is_busy(connection, &target).await? {
        return Ok(Vec::new());
    }
    let mut held = Vec::new();
    for envelope in cockpit_messaging::queued_for(connection, INBOX_PRODUCER, session).await? {
        // The note never stops the owner's message: an inconsistent envelope is logged
        // and left out, as on the card.
        let Some(entry) = audit_entry_of(&envelope.effect_key) else {
            tracing::error!(
                key = envelope.effect_key,
                "a queued coordinator message names no audit entry"
            );
            continue;
        };
        let Some(audited) = cockpit_authorization::audit_entry(connection, entry).await? else {
            tracing::error!(entry, "a queued coordinator message has no audit entry");
            continue;
        };
        held.push(Held {
            envelope: envelope.id,
            view: HeldMessageView {
                message: entry,
                from: audited.audit.actor.as_str().to_owned(),
            },
        });
    }
    Ok(held)
}

/// What the owner's `message` to an agent ends with while coordinator messages sent before it
/// still wait, so the agent knows they exist when the owner's text refers to one (#897): one
/// line, as a typed message must be. An empty message stays empty, to be refused as such.
pub(crate) fn with_overtake_note(message: String, held: &[Held]) -> String {
    if held.is_empty() || message.trim().is_empty() {
        return message;
    }
    let named: Vec<String> = held
        .iter()
        .map(|held| {
            format!(
                "message {} from session {}",
                held.view.message, held.view.from
            )
        })
        .collect();
    let them = if held.len() == 1 { "it" } else { "them" };
    format!(
        "{message} [Torlek] Sent to you before this message and still held: {}. Torlek hands \
         {them} over when your turn ends.",
        named.join(", ")
    )
}

/// Records on each of `held` that the owner's message overtook it, for its card (#897).
pub(crate) async fn record_overtaken(database: &Database, held: &[Held]) -> Result<(), Failed> {
    if held.is_empty() {
        return Ok(());
    }
    let envelopes: Vec<EnvelopeId> = held.iter().map(|held| held.envelope).collect();
    let write = database.write().await?;
    cockpit_messaging::record_overtaken(&write, &envelopes, now()).await?;
    write.commit().await?;
    Ok(())
}

/// Why a message to `target`, whose hook collects it, cannot be handed over at once, as its
/// agent's latest `report` says; `None` when its hook takes it within seconds.
fn collection_wait(target: &str, report: Option<&SessionReport>) -> Option<String> {
    let Some(report) = report else {
        return Some(format!(
            "session {target}'s agent has not reported through its hooks yet; its hook \
             collects it once the agent waits at its prompt"
        ));
    };
    (report.state == Some(AgentState::Working)).then(|| {
        format!("session {target}'s agent is working; its hook hands it over when the turn ends")
    })
}

/// Why a message `target`'s hook collects still waits, from its agent's latest `report`
/// (#703): held while the agent works or a dialog is open, stuck once it has waited at its
/// prompt for [`STUCK_AFTER_SECONDS`] without the hook taking it.
fn collection_note(target: &str, report: Option<&SessionReport>, sent_at: i64, now: i64) -> String {
    let Some(report) = report else {
        return format!("waits for session {target}'s agent to report through its hooks");
    };
    if report.question_source == Some(QuestionSource::Dialog) {
        return format!("held: session {target} has a dialog open for the owner");
    }
    if report.state == Some(AgentState::Working) {
        return format!("held until session {target}'s turn ends; its hook then hands it over");
    }
    // Counted from the send, not the latest report: an idle agent re-reports, and a moving
    // baseline would flip the note between stuck and not stuck on each one, so `--watch`
    // would reprint it (#769). The seconds are left out of the note for the same reason.
    // A message sent during a turn reads stuck for the moments before its hook takes it.
    if now - sent_at > STUCK_AFTER_SECONDS {
        return format!(
            "stuck: session {target} waits at its prompt, but its hook has not collected it \
             for over {STUCK_AFTER_SECONDS} s: its Claude Code may not run asyncRewake hooks, or it started \
             before Torlek collected messages; restarting the session starts its hook"
        );
    }
    format!("session {target}'s hook collects it within seconds")
}

/// The outbox key of the message audited as `entry`: one envelope per audited message.
fn message_key(entry: i64) -> String {
    format!("coordinator-message:{entry}")
}

/// The line the target's agent reads: who sent it and that it is not the owner speaking.
fn message_line(coordinator: &Holder, text: &str) -> String {
    format!(
        "[Torlek] Message from session {} (a collaborator, not the owner): {text}",
        coordinator.as_str()
    )
}

fn check_message_text(text: &str) -> Result<(), Failed> {
    if text.trim().is_empty() {
        return Err(invalid_argument("the message is empty"));
    }
    let count = text.chars().count();
    if count > MESSAGE_MAX_CHARS {
        return Err(invalid_argument(&format!(
            "the message is {count} characters; at most {MESSAGE_MAX_CHARS}"
        )));
    }
    if text.chars().any(char::is_control) {
        return Err(invalid_argument(
            "the message is one line, without control characters",
        ));
    }
    Ok(())
}

fn is_over(session: &Session) -> bool {
    matches!(session.state, SessionState::Ended | SessionState::Failed)
}

/// Nothing reads a message to, or a grant for, a session that ended or failed.
fn refuse_if_over(session: &Session) -> Result<(), Failed> {
    if !is_over(session) {
        return Ok(());
    }
    Err(Failed(ProtocolError::new(
        ErrorCode::Conflict,
        format!("session {} has {}", session.id, session.state),
    )))
}

fn coordinator_permission(name: &str) -> Result<Permission, Failed> {
    name.parse::<Permission>()
        .ok()
        .filter(|permission| permission.is_coordinator())
        .ok_or_else(|| {
            invalid_argument(&format!(
                "{name:?} is not a coordinator grant: observe-sessions, message-sessions, \
                 start-sessions or end-sessions"
            ))
        })
}

fn holder(session: &str) -> Result<Holder, Failed> {
    Holder::new(session).map_err(|error| invalid_argument(&error.to_string()))
}

/// The sender's session, read-only when its launch is bound already.
async fn identified(
    database: &Database,
    sender: &Sender,
    launch_token: Option<String>,
) -> Result<Holder, Failed> {
    let read = database.read().await?;
    if let Some(session) = sender
        .identify_bound(&read, launch_token.as_deref())
        .await?
    {
        return Ok(session);
    }
    drop(read);
    let write = database.write().await?;
    let session = sender.identify(&write, launch_token).await?;
    write.commit().await?;
    Ok(session)
}

async fn granted(
    connection: &Connection,
    coordinator: &Holder,
    outcome: Outcome,
) -> Result<GrantId, Failed> {
    match outcome {
        Outcome::Allowed(Basis::Grant(grant)) => Ok(grant),
        Outcome::Allowed(Basis::Owner) => unreachable!("a session is never allowed as the owner"),
        Outcome::Refused(refusal) => Err(not_granted(connection, coordinator, &refusal).await),
    }
}

/// The refusal of a coordinator verb, naming the owner's command that gives its grant: it
/// holds no such grant, or holds it over another scope than the one asked for (#1131).
async fn not_granted(connection: &Connection, coordinator: &Holder, refusal: &Refusal) -> Failed {
    let permission = match refusal {
        Refusal::NoGrant { permission, .. } | Refusal::OwnerOnly { permission } => *permission,
    };
    let held = match cockpit_authorization::active_grants(connection, coordinator).await {
        Ok(held) => held,
        Err(error) => return error.into(),
    };
    let held_scope = held
        .into_iter()
        .find(|grant| grant.permission == permission)
        .map(|grant| grant.scope);
    let coordinator = coordinator.as_str();
    // The scope asked is not named: it would tell a coordinator another project's sessions.
    match held_scope {
        Some(held_scope) => refused(
            ErrorCode::NotAuthorized,
            format!("session {coordinator}'s {permission} grant covers {held_scope} only"),
            format!(
                "the owner runs `torlek session grant {coordinator} {permission} --all-projects` to cover every project"
            ),
            UnblockedBy::Owner,
        ),
        None => refused(
            ErrorCode::NotAuthorized,
            format!("session {coordinator} holds no {permission} grant"),
            format!("the owner runs `torlek session grant {coordinator} {permission}`"),
            UnblockedBy::Owner,
        ),
    }
}

/// The scope a coordinator grant must cover to act on `session`: its project's (#1131).
async fn scope_of(connection: &Connection, session: &Holder) -> Result<Scope, Failed> {
    let session =
        cockpit_sessions::session(connection, &SessionId::parse(session.as_str())?).await?;
    Ok(cockpit_sessions::coordinator_scope(connection, &session).await?)
}

fn project_scope(name: &str) -> Result<Scope, Failed> {
    Ok(Scope::Project(
        cockpit_authorization::ProjectName::new(name).map_err(|error| internal(&error))?,
    ))
}

fn refused(code: ErrorCode, message: String, action: String, by: UnblockedBy) -> Failed {
    Failed(ProtocolError {
        code,
        message,
        unblock: Some(Unblock { action, by }),
    })
}

fn invalid_argument(message: &str) -> Failed {
    Failed(ProtocolError::new(ErrorCode::InvalidArgument, message))
}

impl From<cockpit_authorization::Error> for Failed {
    fn from(error: cockpit_authorization::Error) -> Failed {
        match error {
            cockpit_authorization::Error::InvalidStartTerms { reason } => invalid_argument(reason),
            error => internal(&error),
        }
    }
}

impl From<cockpit_messaging::Error> for Failed {
    fn from(error: cockpit_messaging::Error) -> Failed {
        internal(&error)
    }
}

#[cfg(test)]
mod tests {
    use cockpit_authorization::SessionId;
    use cockpit_sessions::{AgentState, SessionReport};

    use super::{STUCK_AFTER_SECONDS, collection_note};

    const SENT_AT: i64 = 1_000;

    fn waiting_report(reported_at: i64) -> SessionReport {
        SessionReport {
            session: SessionId::new("s0123456789ab").unwrap(),
            state: Some(AgentState::Waiting),
            label: None,
            doing: None,
            needs_input: None,
            question_source: None,
            covered_question: None,
            outcome: None,
            reported_at,
            launch: None,
            question_asked_at: None,
            question_escalated_at: None,
            context: None,
        }
    }

    /// #769: `agent sessions --watch` reprints a line only when it changes, so the note of a
    /// stuck message must read the same however long the wait has gone on and however often
    /// the target re-reports while idle.
    #[test]
    fn a_stuck_note_reads_the_same_across_polls_and_idle_re_reports() {
        let note_at = |reported_at: i64, polled_at: i64| {
            collection_note(
                "s0123456789ab",
                Some(&waiting_report(reported_at)),
                SENT_AT,
                polled_at,
            )
        };
        let first = note_at(SENT_AT, SENT_AT + STUCK_AFTER_SECONDS + 1);
        assert!(first.starts_with("stuck: "), "{first}");
        assert!(first.contains("for over 60 s"), "{first}");

        for (reported_at, polled_at) in [
            (SENT_AT, SENT_AT + 90),
            (SENT_AT + 70, SENT_AT + 71),
            (SENT_AT + 300, SENT_AT + 302),
        ] {
            assert_eq!(
                note_at(reported_at, polled_at),
                first,
                "reported at +{}, polled at +{}",
                reported_at - SENT_AT,
                polled_at - SENT_AT
            );
        }
    }

    #[test]
    fn a_message_is_not_stuck_before_the_limit() {
        let note = collection_note(
            "s0123456789ab",
            Some(&waiting_report(SENT_AT)),
            SENT_AT,
            SENT_AT + STUCK_AFTER_SECONDS,
        );
        assert_eq!(
            note,
            "session s0123456789ab's hook collects it within seconds"
        );
    }
}
