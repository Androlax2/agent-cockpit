//! Network requests (#674): files the destinations each sandboxed session's launcher refused
//! as requests for the owner, carries out the owner's decisions on the host-only socket, and
//! keeps each launcher's decisions file current. The rules of the rows live in
//! `cockpit-sandbox`; this file maps them to the wire and moves the files.
//!
//! Requests come only from launchers, through a file in the session's mount folder no agent
//! reaches, and only ask: an entry is added by the owner's decision alone.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cockpit_protocol::{
    Command, DecidedThroughView, ErrorCode, NetworkDecisionView, NetworkGrantView,
    NetworkRequestStateView, NetworkRequestView, NetworkSubjectView, NetworkTargetKindView,
    ProtocolError, Reply, Unblock, UnblockedBy,
};
use cockpit_sandbox::{
    EgressDecisions, EgressRequests, NetworkError, NetworkGrant, NetworkRefusal, NetworkRequest,
    NetworkRequestState, OwnerClient, OwnerDecision, Sandbox, Subject,
};
use cockpit_turso::Database;
use cockpit_turso::sql::Connection;
use tokio::sync::Notify;

use crate::approvals::{Failed, internal, job, now};
use crate::routes::{Route, WorkClass};

/// How often the launchers' requests files are read: a refused destination reaches the
/// owner's list within this.
const COLLECT_INTERVAL: Duration = Duration::from_millis(250);

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
            NetworkRefusal::TooSoon { wait_seconds } => (
                ErrorCode::Refused,
                format!("read the request, then allow it again in {wait_seconds} s"),
            ),
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
                let live = live_sessions(&read).await?;
                let requests = cockpit_sandbox::asking_network_requests(&read).await?;
                Ok(Reply::NetworkRequests {
                    requests: requests
                        .iter()
                        .filter(|request| live.contains_key(subject_session(&request.subject)))
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

/// Files what the launchers refused and, at the start and after each decision, rewrites
/// every live session's decisions file, until the daemon stops. One pass's failure is
/// logged and the next pass tries again.
pub(crate) async fn keep_forever(database: Arc<Database>, sandbox: Sandbox, decided: Arc<Notify>) {
    let mut seen = HashMap::new();
    // At the start: a decision committed just before a crash may not have reached its file.
    let mut is_decided = true;
    loop {
        if is_decided && let Err(error) = write_decisions(&database, &sandbox).await {
            tracing::warn!(%error, "the owner's network decisions were not written");
        }
        if let Err(error) = collect(&database, &sandbox, &mut seen).await {
            tracing::warn!(%error, "the sandbox's network requests were not collected");
        }
        is_decided = tokio::select! {
            () = decided.notified() => true,
            () = tokio::time::sleep(COLLECT_INTERVAL) => false,
        };
    }
}

/// Files the destinations each live session's launcher listed since the last pass.
async fn collect(
    database: &Database,
    sandbox: &Sandbox,
    seen: &mut HashMap<String, EgressRequests>,
) -> Result<(), String> {
    let reading = sandbox.clone();
    let listed = tokio::task::spawn_blocking(move || {
        let mut listed = Vec::new();
        for session in reading.staged_sessions()? {
            if let Some(requests) = reading.egress_requests(&session)? {
                listed.push((session, requests));
            }
        }
        Ok::<_, String>(listed)
    })
    .await
    .map_err(|error| error.to_string())??;
    let changed: Vec<(String, EgressRequests)> = listed
        .into_iter()
        .filter(|(session, requests)| seen.get(session) != Some(requests))
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    let write = database.write().await.map_err(|error| error.to_string())?;
    let live = live_sessions(&write)
        .await
        .map_err(|_| "the live sessions could not be read".to_owned())?;
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
            if !is_askable(&asked.host, asked.port) {
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
                tracing::info!(
                    request = filed.id,
                    session,
                    destination = %cockpit_sandbox::destination(&filed.host, filed.port),
                    "a sandboxed session asks the owner for a host"
                );
            }
        }
    }
    write.commit().await.map_err(|error| error.to_string())?;
    seen.extend(changed);
    Ok(())
}

/// A lower-case DNS name of at least two labels and a port: what an entry may name.
fn is_askable(host: &str, port: u16) -> bool {
    port != 0
        && host.contains('.')
        && cockpit_sandbox::normalized_name(host).as_deref() == Some(host)
}

/// Rewrites the decisions file of every live session that has a sandbox folder.
async fn write_decisions(database: &Database, sandbox: &Sandbox) -> Result<(), String> {
    let read = database.read().await.map_err(|error| error.to_string())?;
    let live = live_sessions(&read)
        .await
        .map_err(|_| "the live sessions could not be read".to_owned())?;
    let mut all: Vec<(String, EgressDecisions)> = Vec::with_capacity(live.len());
    for (session, project) in live {
        let subject = Subject::Session {
            session: session.clone(),
        };
        let decisions = cockpit_sandbox::egress_decisions(&read, &project, &subject)
            .await
            .map_err(|error| error.to_string())?;
        all.push((session, decisions));
    }
    drop(read);
    let writing = sandbox.clone();
    tokio::task::spawn_blocking(move || {
        for (session, decisions) in all {
            writing.write_egress_decisions(&session, &decisions)?;
        }
        Ok::<_, String>(())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Every session not ended, by id, with its project's name.
async fn live_sessions(connection: &Connection) -> Result<HashMap<String, String>, Failed> {
    let sessions = cockpit_sessions::sessions(connection, false)
        .await
        .map_err(|error| internal(&error))?;
    let checkouts = cockpit_projects::checkouts(connection, None)
        .await
        .map_err(|error| internal(&error))?;
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

fn subject_session(subject: &Subject) -> &str {
    match subject {
        Subject::Session { session } => session,
    }
}

fn owner_client(through: DecidedThroughView) -> OwnerClient {
    match through {
        DecidedThroughView::Tui => OwnerClient::Tui,
        DecidedThroughView::Cli => OwnerClient::Cli,
    }
}

fn decision_view(decision: OwnerDecision) -> NetworkDecisionView {
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
            id: subject_session(&request.subject).to_owned(),
        },
        project: request.project.clone(),
        kind: NetworkTargetKindView::Host,
        host: request.host.clone(),
        port: request.port,
        target: cockpit_sandbox::destination(&request.host, request.port),
        state: match request.state {
            NetworkRequestState::Asking => NetworkRequestStateView::Asking,
            NetworkRequestState::AllowedOnce => NetworkRequestStateView::AllowedOnce,
            NetworkRequestState::AllowedAlways => NetworkRequestStateView::AllowedAlways,
            NetworkRequestState::Denied => NetworkRequestStateView::Denied,
        },
        asked_at: request.asked_at,
        decision: request.decision.map(decision_view),
        grant: request.grant,
    }
}

fn grant_view(grant: &NetworkGrant) -> NetworkGrantView {
    NetworkGrantView {
        id: grant.id,
        project: grant.project.clone(),
        host: grant.host.clone(),
        port: grant.port,
        target: cockpit_sandbox::destination(&grant.host, grant.port),
        request: grant.request,
        granted: decision_view(grant.granted),
        revoked: grant.revoked.map(decision_view),
    }
}
