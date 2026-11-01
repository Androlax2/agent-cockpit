//! The coordinator kit (#410): an agent's `agent-sessions`, `agent-message` and `agent-start`
//! from the agent socket, the owner's `session-grant`, `session-revoke` and `session-grants`,
//! the delivery of queued messages, and the coordinator notes on session cards. The rules
//! live in `cockpit-authorization` (grants, start terms and cap, audit), `cockpit-approvals`
//! (a start waiting for the owner) and `cockpit-messaging` (the outbox); this file maps and
//! wires.
//!
//! Message delivery types into the target's terminal through `Sessions::submit`, the input
//! path sessions have until provider delivery (#431): the agent reads it at its next prompt.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cockpit_agent_channel::{ProcessError, ProcessIdentity};
use cockpit_approvals::{Approved, NoResult, Runner, StartAsk, StartSession};
use cockpit_authorization::{
    Actor, Audit, AuditEntry, AuditedAction, Basis, GrantId, Outcome, Permission, Refusal, Scope,
    SessionId as Holder, StartAllowance, StartTerms,
};
use cockpit_messaging::{
    Claim, Coalescing, Deliverer, Delivery, DeliveryState, Envelope, Failure, Redelivery,
};
use cockpit_protocol::{
    Command, CoordinatorAuditView, CoordinatorGrantView, ErrorCode, InboundMessageView,
    ObservedSessionView, ProtocolError, Reply, Unblock, UnblockedBy,
};
use cockpit_sessions::{
    Addressed, QuestionSource, Session, SessionId, SessionState, Sessions, StartRequest,
};
use cockpit_turso::Database;
use cockpit_turso::sql::Connection;
use tokio::sync::Notify;

use crate::approvals::{Answer, Failed, Sender, internal, job, now};
use crate::routes::{Route, WorkClass};

/// The outbox producer whose envelopes are coordinator messages.
const PRODUCER: &str = "coordinator";
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
}

/// The route for a coordinator verb from the agent socket. Agent requests run in the bulk
/// lane, like every agent request (#32).
pub(crate) fn route_agent(
    command: Command,
    connector: Result<ProcessIdentity, ProcessError>,
    context: Context,
) -> Route {
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
            Command::AgentStart {
                launch_token,
                dir,
                agent,
                prompt,
                clone,
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
                    },
                )
                .await
            }
            other => unreachable!("routes.rs sends only coordinator verbs here, not {other:?}"),
        }
    });
    Route::Run {
        class: WorkClass::Bulk,
        job: work,
    }
}

/// The route for the owner's coordinator grants; small and the owner's: the control lane.
pub(crate) fn route_owner(command: Command, context: Context) -> Route {
    let work = job(async move {
        match command {
            Command::SessionGrant {
                session,
                permission,
                without_confirmation,
                per_hour,
            } => {
                give(
                    &context,
                    &session,
                    &permission,
                    without_confirmation,
                    per_hour,
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

/// `agent-sessions`: every other session not ended, with its agent's latest report. Read
/// only: the sender's launch is bound with a write only when it is not yet.
async fn observe(context: &Context, sender: &Sender, launch_token: Option<String>) -> Answer {
    let coordinator = identified(&context.database, sender, launch_token).await?;
    let read = context.database.read().await?;
    let decision = cockpit_authorization::evaluate(
        &read,
        &Actor::Session(coordinator.clone()),
        Permission::ObserveSessions,
        &Scope::Owner,
    )
    .await?;
    if let Outcome::Refused(refusal) = decision.outcome {
        return Err(not_granted(&coordinator, &refusal));
    }
    let mut reports: HashMap<String, cockpit_sessions::SessionReport> =
        cockpit_sessions::latest_reports(&read)
            .await?
            .into_iter()
            .map(|report| (report.session.as_str().to_owned(), report))
            .collect();
    drop(read);
    let sessions = context
        .sessions
        .list(false)
        .await?
        .into_iter()
        .filter(|session| session.id.as_str() != coordinator.as_str())
        .map(|session| ObservedSessionView {
            report: reports
                .remove(session.id.as_str())
                .map(crate::agent::to_wire),
            id: session.id.to_string(),
            agent: session.provider.to_string(),
            state: crate::sessions::state_view(session.state),
            working_dir: session.working_dir.to_string_lossy().into_owned(),
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
            "name another session from `cockpit agent sessions`".to_owned(),
            UnblockedBy::Agent,
        ));
    }
    {
        let read = context.database.read().await?;
        let decision = cockpit_authorization::evaluate(
            &read,
            &Actor::Session(coordinator.clone()),
            Permission::MessageSessions,
            &Scope::Session(target_holder.clone()),
        )
        .await?;
        granted(&coordinator, decision.outcome)?;
    }
    let target_session = context.sessions.get(&target_id).await?;
    refuse_if_over(&target_session)?;
    let write = context.database.write().await?;
    let coordinator = sender.identify(&write, launch_token).await?;
    let decision = cockpit_authorization::evaluate(
        &write,
        &Actor::Session(coordinator.clone()),
        Permission::MessageSessions,
        &Scope::Session(target_holder.clone()),
    )
    .await?;
    let grant = granted(&coordinator, decision.outcome)?;
    let report = cockpit_sessions::latest_report(&write, &target_holder).await?;
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
    cockpit_messaging::enqueue(
        &write,
        &Envelope {
            effect_key: message_key(entry),
            producer: PRODUCER.to_owned(),
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
    context.queued.notify_one();
    tracing::info!(
        from = coordinator.as_str(),
        to = target.as_str(),
        audit = entry,
        "a coordinator queued a message"
    );
    Ok(Reply::AgentMessageQueued {
        session: target,
        message: entry,
    })
}

/// `agent-start`: within the hourly cap, either files the start in the `ask run` queue or,
/// when the grant says no confirmation, starts it now. Each counts against the cap once
/// recorded, whether it then starts or not.
async fn start(
    context: &Context,
    sender: &Sender,
    launch_token: Option<String>,
    dir: &str,
    start: StartSession,
) -> Answer {
    let dir = cockpit_approvals::resolve_cwd(dir)?;
    cockpit_approvals::validate_start(&dir, &start)?;
    let write = context.database.write().await?;
    let coordinator = sender.identify(&write, launch_token).await?;
    let at = now();
    let (grant, revision, needs_confirmation) =
        match cockpit_authorization::start_allowance(&write, &coordinator, at).await? {
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
            StartAllowance::Refused(refusal) => return Err(not_granted(&coordinator, &refusal)),
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
        let project = crate::approvals::project_at(&write, &dir).await?;
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
    cockpit_authorization::record_audit(&write, &audit).await?;
    write.commit().await?;
    let started = context.sessions.start(start_request(dir, start)).await?;
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
    match sessions
        .start(start_request(approved.cwd.clone(), start))
        .await
    {
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

fn start_request(dir: PathBuf, start: StartSession) -> StartRequest {
    StartRequest {
        dir,
        provider: start.agent,
        first_message: start.prompt,
        clone_branch: start.clone,
    }
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

/// `session-grant`: gives `session` one coordinator grant over the owner's scope. Giving one
/// it holds changes nothing; giving `start-sessions` with other terms replaces it.
async fn give(
    context: &Context,
    session: &str,
    permission: &str,
    without_confirmation: bool,
    per_hour: Option<u32>,
) -> Answer {
    let permission = coordinator_permission(permission)?;
    let has_terms = without_confirmation || per_hour.is_some();
    if has_terms && permission != Permission::StartSessions {
        return Err(invalid_argument(
            "--without-confirmation and --per-hour are terms of start-sessions only",
        ));
    }
    if without_confirmation && per_hour.is_none() {
        return Err(invalid_argument(
            "--without-confirmation needs --per-hour: starts nobody confirms have an explicit cap",
        ));
    }
    let id = SessionId::parse(session)?;
    refuse_if_over(&context.sessions.get(&id).await?)?;
    let holder = holder(session)?;
    let write = context.database.write().await?;
    let given = if permission == Permission::StartSessions {
        let terms = StartTerms {
            per_hour: per_hour.unwrap_or(StartTerms::DEFAULT.per_hour),
            needs_confirmation: !without_confirmation,
        };
        cockpit_authorization::grant_start_sessions(&write, &holder, terms).await?
    } else {
        cockpit_authorization::grant(&write, &holder, permission, &Scope::Owner).await?
    };
    write.commit().await?;
    tracing::info!(
        session,
        %permission,
        grant = given.id.value(),
        "the owner gave a coordinator grant"
    );
    let read = context.database.read().await?;
    grants_reply(&read, &holder).await
}

/// `session-revoke`: takes back `session`'s coordinator grant; a start it waits on leaves the
/// queue with it.
async fn take_back(context: &Context, session: &str, permission: &str) -> Answer {
    let permission = coordinator_permission(permission)?;
    let holder = holder(session)?;
    let write = context.database.write().await?;
    let Some(held) =
        cockpit_authorization::active_grant(&write, &holder, permission, &Scope::Owner).await?
    else {
        return Err(Failed(ProtocolError::new(
            ErrorCode::NotFound,
            format!("session {session} holds no {permission} grant"),
        )));
    };
    cockpit_authorization::revoke(&write, held.id).await?;
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
            per_hour: grant.start_terms.map(|terms| terms.per_hour),
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
        let rejected = |reason: String| Delivery::Failed(Failure::Rejected { reason });
        let Some(entry) = claim
            .effect_key
            .strip_prefix("coordinator-message:")
            .and_then(|id| id.parse::<i64>().ok())
        else {
            return Err(rejected(format!(
                "{} names no audited message",
                claim.effect_key
            )));
        };
        let target = holder(&claim.destination).map_err(|Failed(error)| rejected(error.message))?;
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
        let Some(entry) = cockpit_authorization::audit_entry(&read, entry)
            .await
            .map_err(|error| not_sent(error.to_string()))?
        else {
            tracing::error!(entry, "a queued coordinator message has no audit entry");
            return Err(rejected(format!("audit entry {entry} is missing")));
        };
        let sender = entry.audit.actor;
        let decision = cockpit_authorization::evaluate(
            &read,
            &Actor::Session(sender.clone()),
            Permission::MessageSessions,
            &Scope::Session(target),
        )
        .await
        .map_err(|error| not_sent(error.to_string()))?;
        match decision.outcome {
            Outcome::Allowed(_) => Ok(sender),
            Outcome::Refused(_) => Err(rejected(format!(
                "session {} no longer holds message-sessions",
                sender.as_str()
            ))),
        }
    }
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
        notes.inbound.insert(
            target.as_str().to_owned(),
            InboundMessageView {
                from: audit.actor.as_str().to_owned(),
                text,
                delivery: envelope.state.as_str().to_owned(),
                sent_at: audit.at,
            },
        );
    }
    Ok(notes)
}

/// The outbox key of the message audited as `entry`: one envelope per audited message.
fn message_key(entry: i64) -> String {
    format!("coordinator-message:{entry}")
}

/// The line the target's agent reads: who sent it and that it is not the owner speaking.
fn message_line(coordinator: &Holder, text: &str) -> String {
    format!(
        "[Cockpit] Message from session {} (a collaborator, not the owner): {text}",
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
                "{name:?} is not a coordinator grant: observe-sessions, message-sessions or \
                 start-sessions"
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

fn granted(coordinator: &Holder, outcome: Outcome) -> Result<GrantId, Failed> {
    match outcome {
        Outcome::Allowed(Basis::Grant(grant)) => Ok(grant),
        Outcome::Allowed(Basis::Owner) => unreachable!("a session is never allowed as the owner"),
        Outcome::Refused(refusal) => Err(not_granted(coordinator, &refusal)),
    }
}

/// The refusal of a coordinator verb without its grant, naming the owner's command that
/// gives it.
fn not_granted(coordinator: &Holder, refusal: &Refusal) -> Failed {
    let permission = match refusal {
        Refusal::NoGrant { permission, .. } | Refusal::OwnerOnly { permission } => permission,
    };
    let coordinator = coordinator.as_str();
    refused(
        ErrorCode::NotAuthorized,
        format!("session {coordinator} holds no {permission} grant"),
        format!("the owner runs `cockpit session grant {coordinator} {permission}`"),
        UnblockedBy::Owner,
    )
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
