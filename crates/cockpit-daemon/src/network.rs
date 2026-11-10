//! Network requests (#674): files the destinations and service records (#880) each sandboxed
//! session's launcher refused as requests for the owner, carries out the owner's decisions on the host-only socket, and
//! keeps each launcher's decisions file current. The rules of the rows live in
//! `cockpit-sandbox`; this file maps them to the wire and moves the files.
//!
//! Requests come from launchers, through a file in the session's mount folder no agent
//! reaches, and from an agent's `torlek ask service` for a service its project declares. Both
//! only ask: an entry is added by the owner's decision alone.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cockpit_agent_channel::{ProcessError, ProcessIdentity};
use cockpit_protocol::{
    Command, DecidedThroughView, ErrorCode, NetworkDecisionView, NetworkGrantView,
    NetworkRequestStateView, NetworkRequestView, NetworkSubjectView, NetworkTargetView,
    ProtocolError, Reply, Unblock, UnblockedBy,
};
use cockpit_sandbox::{
    EgressDecisions, EgressRequests, Forward, NetworkError, NetworkGrant, NetworkRefusal,
    NetworkRequest, NetworkRequestState, NetworkTarget, OwnerClient, OwnerDecision, Sandbox,
    ServiceAsk, Subject,
};
use cockpit_sessions::Sessions;
use cockpit_turso::Database;
use cockpit_turso::sql::Connection;
use tokio::sync::Notify;

use crate::approvals::{Failed, Sender, internal, job, now, shown};
use crate::routes::{Route, WorkClass};

/// How often the launchers' requests files are read: a refused destination reaches the
/// owner's list within this.
const COLLECT_INTERVAL: Duration = Duration::from_millis(250);

/// Every this many passes, about a second, the decisions files are rewritten even without a
/// decision; an unchanged file is left as it is.
const RESYNC_PASSES: u32 = 4;

impl From<NetworkError> for Failed {
    fn from(error: NetworkError) -> Failed {
        let NetworkError::Refused(refusal) = &error else {
            return internal(&error);
        };
        let (code, action) = match refusal {
            NetworkRefusal::NotFound { kind, .. } => (
                ErrorCode::NotFound,
                format!("name a {kind} `torlek network list` or `torlek network grants` shows"),
            ),
            NetworkRefusal::AlreadyDecided { .. } | NetworkRefusal::AlreadyRevoked => (
                ErrorCode::Conflict,
                "nothing to do: it was decided".to_owned(),
            ),
            NetworkRefusal::AlreadyGranted { grant } => (
                ErrorCode::Conflict,
                format!("nothing to do; `torlek network revoke {grant}` takes it back"),
            ),
            NetworkRefusal::TooSoon { wait_seconds } => (
                ErrorCode::Refused,
                format!("read the request, then allow it again in {wait_seconds} s"),
            ),
            // No owner verb names a subject: a check run reaching here is the daemon's bug.
            NetworkRefusal::NotASession => return internal(&error),
        };
        Failed(ProtocolError {
            code,
            message: refusal.to_string(),
            unblock: Some(Unblock {
                action,
                by: UnblockedBy::Owner,
            }),
        })
    }
}

/// The route for the owner's network verbs; `routes.rs` sends only those here, from the
/// host-only socket. `decided` wakes [`keep_forever`] to rewrite the launchers' decisions.
pub(crate) fn route(command: Command, database: Arc<Database>, decided: Arc<Notify>) -> Route {
    let work = job(async move {
        match command {
            Command::NetworkList => {
                let read = database.read().await?;
                let live = live_sessions(&read)
                    .await
                    .map_err(|reason| internal(&reason))?;
                let requests = cockpit_sandbox::asking_network_requests(&read).await?;
                Ok(Reply::NetworkRequests {
                    requests: requests
                        .iter()
                        .filter(|request| live.contains_key(&request.session))
                        .map(request_view)
                        .collect(),
                })
            }
            Command::NetworkAllow {
                request,
                always,
                through,
            } => {
                let write = database.write().await?;
                let allowed = cockpit_sandbox::allow_network_request(
                    &write,
                    request,
                    always,
                    through.map(owner_client),
                    now(),
                )
                .await?;
                write.commit().await?;
                decided.notify_one();
                tracing::info!(request, always, "the owner allowed a network request");
                Ok(request_reply(&allowed))
            }
            Command::NetworkDeny { request, through } => {
                let write = database.write().await?;
                let denied = cockpit_sandbox::deny_network_request(
                    &write,
                    request,
                    through.map(owner_client),
                    now(),
                )
                .await?;
                write.commit().await?;
                decided.notify_one();
                tracing::info!(request, "the owner denied a network request");
                Ok(request_reply(&denied))
            }
            Command::NetworkGrants { project } => {
                let read = database.read().await?;
                let grants = cockpit_sandbox::network_grants(&read, project.as_deref()).await?;
                Ok(Reply::NetworkGrants {
                    grants: grants.iter().map(grant_view).collect(),
                })
            }
            Command::NetworkGrantGithub { project, through } => {
                let write = database.write().await?;
                registered(&write, &project).await?;
                let granted = cockpit_sandbox::grant_github(
                    &write,
                    &project,
                    through.map(owner_client),
                    now(),
                )
                .await?;
                write.commit().await?;
                decided.notify_one();
                tracing::info!(grant = granted.id, %project, "the owner granted github.com");
                Ok(Reply::NetworkGrant {
                    grant: Box::new(grant_view(&granted)),
                })
            }
            Command::NetworkRevoke { grant, through } => {
                let write = database.write().await?;
                let revoked = cockpit_sandbox::revoke_network_grant(
                    &write,
                    grant,
                    through.map(owner_client),
                    now(),
                )
                .await?;
                write.commit().await?;
                decided.notify_one();
                tracing::info!(grant, "the owner revoked a network grant");
                Ok(Reply::NetworkGrant {
                    grant: Box::new(grant_view(&revoked)),
                })
            }
            other => unreachable!("routes.rs sends only network verbs here, not {other:?}"),
        }
    });
    Route::Run {
        class: WorkClass::Control,
        job: work,
    }
}

/// The most characters an agent's reason for a service holds, as for an ask to run.
const REASON_MAX_CHARS: usize = 1000;

/// The route for an agent's `torlek ask service` (#880); `routes.rs` sends only that here,
/// from the agent socket. It asks under the agent's `run-commands` grant, like any ask, and
/// `decided` wakes [`keep_forever`] so the launcher names the new request.
pub(crate) fn route_ask(
    command: Command,
    connector: Result<ProcessIdentity, ProcessError>,
    database: Arc<Database>,
    sessions: Arc<Sessions>,
    decided: Arc<Notify>,
) -> Route {
    let Command::AskService {
        launch_token,
        service,
        reason,
    } = command
    else {
        unreachable!("routes.rs sends only ask-service here, not {command:?}")
    };
    let work = job(async move {
        validate_reason(&reason)?;
        let sender = Sender::read(connector).await;
        let write = database.write().await?;
        let session = sender.identify(&write, launch_token.clone()).await?;
        cockpit_approvals::may_ask(&write, &session).await?;
        write.commit().await?;
        let session_id = cockpit_sessions::SessionId::parse(session.as_str())
            .map_err(|error| internal(&error))?;
        let declared = sessions
            .declared_services(&session_id)
            .await
            .map_err(|error| Failed(crate::sessions::protocol_error(&error)))?;
        let forward = declared_service(&declared, &service)?;
        // Again in the transaction that files: a grant revoked meanwhile asks nothing.
        let write = database.write().await?;
        let session = sender.identify(&write, launch_token).await?;
        cockpit_approvals::may_ask(&write, &session).await?;
        let project = live_sessions(&write)
            .await
            .map_err(|reason| internal(&reason))?
            .remove(session.as_str())
            .ok_or_else(|| internal(&"the asking session holds no checkout"))?;
        let subject = Subject::Session {
            session: session.as_str().to_owned(),
        };
        let asked =
            cockpit_sandbox::ask_service(&write, &subject, &project, forward, &reason, now())
                .await?;
        write.commit().await?;
        if let ServiceAsk::Filed(filed) = &asked {
            decided.notify_one();
            tracing::info!(
                request = filed.id,
                session = filed.session,
                record = filed.target.describe(),
                "an agent asks the owner for a service"
            );
        }
        Ok(ask_reply(&asked))
    });
    Route::Run {
        class: WorkClass::Bulk,
        job: work,
    }
}

fn ask_reply(asked: &ServiceAsk) -> Reply {
    match asked {
        ServiceAsk::Filed(request) | ServiceAsk::Standing(request) => request_reply(request),
        ServiceAsk::Granted(grant) => Reply::NetworkGrant {
            grant: Box::new(grant_view(grant)),
        },
    }
}

/// `service` among the services the session's next launch serves, or a refusal naming them.
fn declared_service<'a>(declared: &'a [Forward], service: &str) -> Result<&'a Forward, Failed> {
    if let Some(forward) = declared.iter().find(|forward| forward.service == service) {
        return Ok(forward);
    }
    let names: Vec<&str> = declared
        .iter()
        .map(|forward| forward.service.as_str())
        .collect();
    let message = if names.is_empty() {
        format!(
            "no declared service {service}: the session's launch serves none, for its project \
             declares no [[services]] or its launches are unsandboxed"
        )
    } else {
        format!(
            "no declared service {service}; the project declares {}",
            names.join(", ")
        )
    };
    Err(Failed(ProtocolError {
        code: ErrorCode::NotFound,
        message,
        unblock: Some(Unblock {
            action: "name a service the project's .torlek/config.toml declares under \
                     [[services]]"
                .to_owned(),
            by: UnblockedBy::Agent,
        }),
    }))
}

fn validate_reason(reason: &str) -> Result<(), Failed> {
    let problem = if reason.trim().is_empty() {
        "is empty; say what the service is for and why it is needed now".to_owned()
    } else if reason.chars().count() > REASON_MAX_CHARS {
        format!("is longer than {REASON_MAX_CHARS} characters")
    } else if reason.contains('\0') {
        "contains a NUL character".to_owned()
    } else {
        return Ok(());
    };
    Err(Failed(ProtocolError {
        code: ErrorCode::InvalidArgument,
        message: format!("the reason {problem}"),
        unblock: Some(Unblock {
            action: "ask again with --reason TEXT".to_owned(),
            by: UnblockedBy::Agent,
        }),
    }))
}

/// Files what the launchers refused and rewrites every live session's decisions file: at
/// the start, after each decision, after filing a request, so its launcher names it, and
/// each [`RESYNC_PASSES`] passes so that a launch that
/// staged decisions read before one was taken gets it too. Until the daemon stops; one
/// pass's failure is logged and the next pass tries again.
pub(crate) async fn keep_forever(database: Arc<Database>, sandbox: Sandbox, decided: Arc<Notify>) {
    let mut seen = HashMap::new();
    // At the start: a decision committed just before a crash may not have reached its file.
    let mut is_decided = true;
    let mut passes: u32 = 0;
    loop {
        if is_decided || passes.is_multiple_of(RESYNC_PASSES) {
            write_decisions(&database, &sandbox).await;
        }
        let is_filed = match collect(&database, &sandbox, &mut seen).await {
            Ok(is_filed) => is_filed,
            Err(error) => {
                tracing::warn!(%error, "the sandbox's network requests were not collected");
                false
            }
        };
        passes = passes.wrapping_add(1);
        // A new request reaches its launcher at once, so the agent's retry names it (#966).
        if is_filed {
            is_decided = true;
            continue;
        }
        is_decided = tokio::select! {
            () = decided.notified() => true,
            () = tokio::time::sleep(COLLECT_INTERVAL) => false,
        };
    }
}

/// Files the destinations each live session's launcher listed since the last pass; true when
/// it filed a new request. The requests files of ended sessions are not read, and what was
/// seen of them is forgotten.
async fn collect(
    database: &Database,
    sandbox: &Sandbox,
    seen: &mut HashMap<String, EgressRequests>,
) -> Result<bool, String> {
    let read = database.read().await.map_err(|error| error.to_string())?;
    let live = live_sessions(&read).await?;
    drop(read);
    seen.retain(|session, _| live.contains_key(session));
    let reading = sandbox.clone();
    let sessions: Vec<String> = live.keys().cloned().collect();
    let listed = tokio::task::spawn_blocking(move || {
        let mut listed = Vec::new();
        for session in sessions {
            match reading.egress_requests(&session) {
                Ok(requests) => listed.push((session, requests)),
                // One bad file does not stop the others' requests.
                Err(error) => {
                    tracing::warn!(session, %error, "a requests file was not read");
                }
            }
        }
        listed
    })
    .await
    .map_err(|error| error.to_string())?;
    // A relaunch removes the file: what was seen of the last launch is forgotten with it.
    let listed: Vec<(String, EgressRequests)> = listed
        .into_iter()
        .filter_map(|(session, requests)| match requests {
            Some(requests) => Some((session, requests)),
            None => {
                seen.remove(&session);
                None
            }
        })
        .collect();
    let changed: Vec<(String, EgressRequests)> = listed
        .into_iter()
        .filter(|(session, requests)| seen.get(session) != Some(requests))
        .collect();
    if changed.is_empty() {
        return Ok(false);
    }
    let mut is_filed = false;
    let write = database.write().await.map_err(|error| error.to_string())?;
    // Again inside the write: a session that ended since asks nothing.
    let live = live_sessions(&write).await?;
    let at = now();
    for (session, requests) in &changed {
        let Some(project) = live.get(session) else {
            continue;
        };
        let subject = Subject::Session {
            session: session.clone(),
        };
        if requests.subject != subject {
            tracing::warn!(session, "a requests file names another subject; ignored");
            continue;
        }
        for asked in requests
            .asked
            .iter()
            .take(cockpit_sandbox::MAX_ASKED_DESTINATIONS)
        {
            if !cockpit_sandbox::is_askable_destination(&asked.host, asked.port) {
                tracing::warn!(
                    session,
                    host = asked.host,
                    "a refused host is not a DNS name"
                );
                continue;
            }
            if let Some(filed) =
                cockpit_sandbox::file_network_request(&write, &subject, project, asked, at)
                    .await
                    .map_err(|error| error.to_string())?
            {
                is_filed = true;
                tracing::info!(
                    request = filed.id,
                    session,
                    destination = filed.target.describe(),
                    "a sandboxed session asks the owner for a host"
                );
            }
        }
        for asked in requests.services.iter().take(cockpit_sandbox::MAX_FORWARDS) {
            if !asked.is_askable() {
                tracing::warn!(session, "a refused service names no service record");
                continue;
            }
            if let Some(filed) =
                cockpit_sandbox::file_service_request(&write, &subject, project, asked, at)
                    .await
                    .map_err(|error| error.to_string())?
            {
                is_filed = true;
                tracing::info!(
                    request = filed.id,
                    session,
                    record = filed.target.describe(),
                    "a sandboxed session asks the owner for a service"
                );
            }
        }
    }
    write.commit().await.map_err(|error| error.to_string())?;
    seen.extend(changed);
    Ok(is_filed)
}

/// Rewrites the decisions file of every live session that has a sandbox folder. A session
/// whose decisions cannot be read or written is logged and the others still get theirs.
async fn write_decisions(database: &Database, sandbox: &Sandbox) {
    let all = match decisions_of_live_sessions(database).await {
        Ok(all) => all,
        Err(error) => {
            tracing::warn!(%error, "the owner's network decisions were not read");
            return;
        }
    };
    let writing = sandbox.clone();
    let written = tokio::task::spawn_blocking(move || {
        for (session, decisions) in all {
            if let Err(error) = writing.write_egress_decisions(&session, &decisions) {
                tracing::warn!(session, %error, "the owner's network decisions were not written");
            }
        }
    })
    .await;
    if let Err(error) = written {
        tracing::warn!(%error, "the owner's network decisions were not written");
    }
}

async fn decisions_of_live_sessions(
    database: &Database,
) -> Result<Vec<(String, EgressDecisions)>, String> {
    let read = database.read().await.map_err(|error| error.to_string())?;
    let live = live_sessions(&read).await?;
    let mut all = Vec::with_capacity(live.len());
    for (session, project) in live {
        let subject = Subject::Session {
            session: session.clone(),
        };
        match cockpit_sandbox::egress_decisions(&read, &project, &subject).await {
            Ok(decisions) => all.push((session, decisions)),
            Err(error) => {
                tracing::warn!(session, %error, "the owner's network decisions were not read");
            }
        }
    }
    Ok(all)
}

/// Every session not ended, by id, with its project's name.
async fn live_sessions(connection: &Connection) -> Result<HashMap<String, String>, String> {
    let sessions = cockpit_sessions::sessions(connection, false)
        .await
        .map_err(|error| format!("the live sessions could not be read: {error}"))?;
    let checkouts = cockpit_projects::checkouts(connection, None)
        .await
        .map_err(|error| format!("the checkouts could not be read: {error}"))?;
    Ok(sessions
        .into_iter()
        .filter_map(|session| {
            let checkout = checkouts
                .iter()
                .find(|checkout| checkout.id == session.checkout)?;
            Some((
                session.id.as_str().to_owned(),
                checkout.project.as_str().to_owned(),
            ))
        })
        .collect())
}

pub(crate) fn owner_client(through: DecidedThroughView) -> OwnerClient {
    match through {
        DecidedThroughView::Tui => OwnerClient::Tui,
        DecidedThroughView::Cli => OwnerClient::Cli,
    }
}

pub(crate) fn decision_view(decision: OwnerDecision) -> NetworkDecisionView {
    NetworkDecisionView {
        at: decision.at,
        through: decision.through.map(|through| match through {
            OwnerClient::Tui => DecidedThroughView::Tui,
            OwnerClient::Cli => DecidedThroughView::Cli,
        }),
    }
}

fn request_reply(request: &NetworkRequest) -> Reply {
    Reply::NetworkRequest {
        request: Box::new(request_view(request)),
    }
}

fn request_view(request: &NetworkRequest) -> NetworkRequestView {
    NetworkRequestView {
        id: request.id,
        subject: NetworkSubjectView::Session {
            id: request.session.clone(),
        },
        project: request.project.clone(),
        asks: target_view(&request.target),
        target: request.target.describe(),
        state: match request.state {
            NetworkRequestState::Asking => NetworkRequestStateView::Asking,
            NetworkRequestState::AllowedOnce => NetworkRequestStateView::AllowedOnce,
            NetworkRequestState::AllowedAlways => NetworkRequestStateView::AllowedAlways,
            NetworkRequestState::Denied => NetworkRequestStateView::Denied,
        },
        asked_at: request.asked_at,
        decision: request.decision.map(decision_view),
        grant: request.grant,
        shown_reason: request.reason.as_deref().map(shown),
    }
}

/// Refuses a name no registered project holds: a grant keyed by a mistyped name would sit
/// unseen until a project of that name is added.
async fn registered(connection: &Connection, project: &str) -> Result<(), Failed> {
    match cockpit_projects::project_named(connection, project).await {
        Ok(_) => Ok(()),
        Err(cockpit_projects::Error::UnknownProject { .. }) => Err(Failed(ProtocolError {
            code: ErrorCode::NotFound,
            message: format!("no registered project {project}"),
            unblock: Some(Unblock {
                action: "name a project `torlek project list` shows".to_owned(),
                by: UnblockedBy::Owner,
            }),
        })),
        Err(error) => Err(internal(&error)),
    }
}

fn grant_view(grant: &NetworkGrant) -> NetworkGrantView {
    NetworkGrantView {
        id: grant.id,
        project: grant.project.clone(),
        allows: target_view(&grant.target),
        target: grant.target.describe(),
        request: grant.request,
        granted: decision_view(grant.granted),
        revoked: grant.revoked.map(decision_view),
    }
}

fn target_view(target: &NetworkTarget) -> NetworkTargetView {
    match target {
        NetworkTarget::Host { host, port } => NetworkTargetView::Host {
            host: host.clone(),
            port: *port,
        },
        NetworkTarget::Service { service, .. } => NetworkTargetView::Service {
            service: service.clone(),
        },
    }
}
