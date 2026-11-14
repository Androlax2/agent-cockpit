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
    CONTAINERS_SERVICE, EgressDecisions, EgressRequests, EngineRelay, Forward, NetworkError,
    NetworkGrant, NetworkRefusal, NetworkRequest, NetworkRequestState, NetworkTarget, OwnerClient,
    OwnerDecision, Sandbox, ServiceAsk, Subject,
};
use cockpit_sessions::Sessions;
use cockpit_turso::sql::Connection;
use cockpit_turso::{Database, WriteTransaction};
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
            NetworkRefusal::OnceForACheck | NetworkRefusal::OnceForContainers => (
                ErrorCode::Refused,
                "allow it always (`--always`, a in the TUI) or deny it".to_owned(),
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
                        .filter(|request| match &request.subject {
                            Subject::Session { session } => live.contains_key(session),
                            // A run ends long before the owner reads its request, which
                            // stands for its check's next runs (#352); a project's containers
                            // keep asking while its engine runs (#390).
                            Subject::Check { .. } | Subject::Containers { .. } => true,
                        })
                        .map(request_view)
                        .collect(),
                })
            }
            Command::NetworkDecided => {
                let read = database.read().await?;
                let requests = cockpit_sandbox::decided_network_requests(&read).await?;
                Ok(Reply::NetworkRequests {
                    requests: requests.iter().map(request_view).collect(),
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
    containers: crate::containers::Host,
) -> Route {
    let Command::AskService {
        launch_token,
        service,
        port,
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
        let session_project = asking_project(&write, session.as_str()).await?;
        write.commit().await?;
        let forward = if service == CONTAINERS_SERVICE {
            let port = port.ok_or_else(needs_port)?;
            // Outside a transaction: the engine may take its whole deadline to answer.
            crate::containers::container_port(
                &database,
                &containers,
                &session_project,
                session.as_str(),
                port,
            )
            .await?
        } else {
            if port.is_some() {
                return Err(port_for_containers_only(&service));
            }
            let session_id = cockpit_sessions::SessionId::parse(session.as_str())
                .map_err(|error| internal(&error))?;
            let declared = sessions
                .declared_services(&session_id)
                .await
                .map_err(|error| Failed(crate::sessions::protocol_error(&error)))?;
            declared_service(&declared, &service)?.clone()
        };
        // Again in the transaction that files: a grant revoked meanwhile asks nothing.
        let write = database.write().await?;
        let session = sender.identify(&write, launch_token).await?;
        cockpit_approvals::may_ask(&write, &session).await?;
        let project = asking_project(&write, session.as_str()).await?;
        if project != session_project {
            return Err(internal(&"the asking session moved to another project"));
        }
        let subject = Subject::Session {
            session: session.as_str().to_owned(),
        };
        let asked =
            cockpit_sandbox::ask_service(&write, &subject, &project, &forward, &reason, now())
                .await?;
        write.commit().await?;
        if let ServiceAsk::Filed(filed) = &asked {
            decided.notify_one();
            tracing::info!(
                request = filed.id,
                subject = ?filed.subject,
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

/// The project of the asking session's checkout.
async fn asking_project(connection: &Connection, session: &str) -> Result<String, Failed> {
    live_sessions(connection)
        .await
        .map_err(|reason| internal(&reason))?
        .remove(session)
        .ok_or_else(|| internal(&"the asking session holds no checkout"))
}

fn needs_port() -> Failed {
    Failed(ProtocolError {
        code: ErrorCode::InvalidArgument,
        message: format!("the {CONTAINERS_SERVICE} service needs the port to open"),
        unblock: Some(Unblock {
            action: format!(
                "ask again with `torlek ask service {CONTAINERS_SERVICE} --port PORT --reason TEXT`, \
                 PORT the one the container publishes"
            ),
            by: UnblockedBy::Agent,
        }),
    })
}

fn port_for_containers_only(service: &str) -> Failed {
    Failed(ProtocolError {
        code: ErrorCode::InvalidArgument,
        message: format!(
            "--port names a container port; the declared service {service} opens where the \
             project declares it"
        ),
        unblock: Some(Unblock {
            action: format!("ask again without --port, or for `{CONTAINERS_SERVICE} --port PORT`"),
            by: UnblockedBy::Agent,
        }),
    })
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
pub(crate) async fn keep_forever(
    database: Arc<Database>,
    sandbox: Sandbox,
    decided: Arc<Notify>,
    containers: crate::containers::Host,
) {
    let mut seen = HashMap::new();
    // At the start: a decision committed just before a crash may not have reached its file.
    let mut is_decided = true;
    let mut passes: u32 = 0;
    loop {
        if is_decided || passes.is_multiple_of(RESYNC_PASSES) {
            write_decisions(&database, &sandbox, &containers).await;
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

/// Files the destinations each live subject's launcher listed since the last pass; true when
/// it filed a new request. The requests files of ended sessions and sealed check runs are not
/// read, and what was seen of them is forgotten.
async fn collect(
    database: &Database,
    sandbox: &Sandbox,
    seen: &mut HashMap<Subject, EgressRequests>,
) -> Result<bool, String> {
    let read = database.read().await.map_err(|error| error.to_string())?;
    let live = live_subjects(&read).await?;
    drop(read);
    seen.retain(|subject, _| live.contains_key(subject));
    let reading = sandbox.clone();
    let subjects: Vec<Subject> = live.keys().cloned().collect();
    let listed = tokio::task::spawn_blocking(move || {
        let mut listed = Vec::new();
        for subject in subjects {
            match requests_of(&reading, &subject) {
                Ok(requests) => listed.push((subject, requests)),
                // One bad file does not stop the others' requests.
                Err(error) => {
                    tracing::warn!(?subject, %error, "a requests file was not read");
                }
            }
        }
        listed
    })
    .await
    .map_err(|error| error.to_string())?;
    // A relaunch removes the file: what was seen of the last launch is forgotten with it.
    let listed: Vec<(Subject, EgressRequests)> = listed
        .into_iter()
        .filter_map(|(subject, requests)| match requests {
            Some(requests) => Some((subject, requests)),
            None => {
                seen.remove(&subject);
                None
            }
        })
        .collect();
    let changed: Vec<(Subject, EgressRequests)> = listed
        .into_iter()
        .filter(|(subject, requests)| seen.get(subject) != Some(requests))
        .collect();
    if changed.is_empty() {
        return Ok(false);
    }
    let mut is_filed = false;
    let write = database.write().await.map_err(|error| error.to_string())?;
    // Again inside the write: a session that ended or a run sealed since asks nothing.
    let live = live_subjects(&write).await?;
    let at = now();
    for (subject, requests) in &changed {
        let Some(project) = live.get(subject) else {
            continue;
        };
        is_filed |= file_listed(&write, subject, project, requests, at)
            .await
            .map_err(|error| error.to_string())?;
    }
    write.commit().await.map_err(|error| error.to_string())?;
    seen.extend(changed);
    Ok(is_filed)
}

/// Files what `subject`'s launcher listed in `requests` as requests of `project` for the
/// owner, in the caller's transaction; true when it filed a new one. A file naming another
/// subject, a host that is no DNS name and a check run's service are logged and skipped.
pub(crate) async fn file_listed(
    write: &WriteTransaction<'_>,
    subject: &Subject,
    project: &str,
    requests: &EgressRequests,
    at: i64,
) -> Result<bool, NetworkError> {
    if requests.subject != *subject {
        tracing::warn!(?subject, "a requests file names another subject; ignored");
        return Ok(false);
    }
    let mut is_filed = false;
    for asked in requests
        .asked
        .iter()
        .take(cockpit_sandbox::MAX_ASKED_DESTINATIONS)
    {
        if !cockpit_sandbox::is_askable_destination(&asked.host, asked.port) {
            tracing::warn!(
                ?subject,
                host = asked.host,
                "a refused host is not a DNS name"
            );
            continue;
        }
        if let Some(filed) =
            cockpit_sandbox::file_network_request(write, subject, project, asked, at).await?
        {
            is_filed = true;
            tracing::info!(
                request = filed.id,
                ?subject,
                destination = filed.target.describe(),
                "a sandboxed launcher asks the owner for a host"
            );
        }
    }
    if matches!(subject, Subject::Check { .. } | Subject::Containers { .. }) {
        // A check run and an engine's proxy are given no forward, so neither has a service to
        // close.
        if !requests.services.is_empty() {
            tracing::warn!(
                ?subject,
                "a requests file without forwards lists services; ignored"
            );
        }
        return Ok(is_filed);
    }
    for asked in requests.services.iter().take(cockpit_sandbox::MAX_FORWARDS) {
        if !asked.is_askable() {
            tracing::warn!(?subject, "a refused service names no service record");
            continue;
        }
        if let Some(filed) =
            cockpit_sandbox::file_service_request(write, subject, project, asked, at).await?
        {
            is_filed = true;
            tracing::info!(
                request = filed.id,
                ?subject,
                record = filed.target.describe(),
                "a sandboxed session asks the owner for a service"
            );
        }
    }
    Ok(is_filed)
}

/// What `subject`'s launcher refused, from its own mount folder; `None` when it refused
/// nothing yet.
fn requests_of(sandbox: &Sandbox, subject: &Subject) -> Result<Option<EgressRequests>, String> {
    match subject {
        Subject::Session { session } => sandbox.egress_requests(session),
        Subject::Check { run, .. } => sandbox.check_egress_requests(run),
        // TODO(#390 part 6c): read the engine proxy's folder once the daemon runs one.
        Subject::Containers { project } => Err(format!(
            "the daemon runs no proxy for the containers of {project} yet"
        )),
    }
}

/// Rewrites the decisions file of every live session and sandboxed check run that has a
/// sandbox folder. One whose decisions cannot be read or written is logged and the others
/// still get theirs.
async fn write_decisions(
    database: &Database,
    sandbox: &Sandbox,
    containers: &crate::containers::Host,
) {
    let all = match decisions_of_live_subjects(database, containers).await {
        Ok(all) => all,
        Err(error) => {
            tracing::warn!(%error, "the owner's network decisions were not read");
            return;
        }
    };
    let writing = sandbox.clone();
    let written = tokio::task::spawn_blocking(move || {
        for (subject, decisions) in all {
            let written = match &subject {
                Subject::Session { session } => writing.write_egress_decisions(session, &decisions),
                Subject::Check { run, .. } => writing.write_check_egress_decisions(run, &decisions),
                // TODO(#390 part 6c): write the engine proxy's folder once the daemon runs one.
                Subject::Containers { project } => Err(format!(
                    "the daemon runs no proxy for the containers of {project} yet"
                )),
            };
            if let Err(error) = written {
                tracing::warn!(?subject, %error, "the owner's network decisions were not written");
            }
        }
    })
    .await;
    if let Err(error) = written {
        tracing::warn!(%error, "the owner's network decisions were not written");
    }
}

async fn decisions_of_live_subjects(
    database: &Database,
    containers: &crate::containers::Host,
) -> Result<Vec<(Subject, EgressDecisions)>, String> {
    let read = database.read().await.map_err(|error| error.to_string())?;
    let mut all = decisions_of_check_runs(&read).await?;
    let live = live_sessions(&read).await?;
    let held = cockpit_sessions::live_port_blocks(&read)
        .await
        .map_err(|error| format!("the sessions' port blocks could not be read: {error}"))?;
    all.reserve(live.len());
    let passwd = tokio::sync::OnceCell::new();
    // A project's sessions share its engine: looked up once a pass.
    let mut relays: HashMap<String, Result<Option<EngineRelay>, String>> = HashMap::new();
    for (session, project) in live {
        let subject = Subject::Session {
            session: session.clone(),
        };
        let mut decisions = match cockpit_sandbox::egress_decisions(&read, &project, &subject).await
        {
            Ok(decisions) => decisions,
            Err(error) => {
                tracing::warn!(session, %error, "the owner's network decisions were not read");
                continue;
            }
        };
        // A port always allowed for the project opens in the session whose block holds it,
        // not in the others (#1371).
        cockpit_sessions::keep_own_container_ports(
            &mut decisions.services,
            &held,
            &project,
            &session,
        );
        if decisions.services.live_forwards().next().is_some() {
            // Without it the launcher refuses each container port, naming the engine.
            if !relays.contains_key(&project) {
                let found = match passwd.get_or_try_init(crate::containers::read_passwd).await {
                    Ok(accounts) => {
                        crate::containers::engine_relay(&read, containers, accounts, &project).await
                    }
                    Err(error) => Err(error),
                };
                relays.insert(project.clone(), found);
            }
            match &relays[&project] {
                Ok(engine) => decisions.services.engine.clone_from(engine),
                Err(error) => {
                    tracing::warn!(session, %error, "the project's engine was not found");
                }
            }
        }
        all.push((subject, decisions));
    }
    Ok(all)
}

/// The decisions of each sandboxed check run not yet sealed (#352): the project's host
/// grants and what the owner allowed or denied for the run. A check is given no forward, so
/// service grants are left out.
async fn decisions_of_check_runs(
    read: &Connection,
) -> Result<Vec<(Subject, EgressDecisions)>, String> {
    let mut all = Vec::new();
    for (subject, project) in live_check_runs(read).await? {
        match cockpit_sandbox::egress_decisions(read, &project, &subject).await {
            Ok(decisions) => all.push((
                subject,
                EgressDecisions {
                    services: cockpit_sandbox::ServiceDecisions::default(),
                    ..decisions
                },
            )),
            Err(error) => {
                tracing::warn!(?subject, %error, "the owner's network decisions were not read");
            }
        }
    }
    Ok(all)
}

/// Every subject whose launcher may still ask the owner: each live session and each
/// sandboxed check run not yet sealed (#352), with its project's name.
async fn live_subjects(connection: &Connection) -> Result<HashMap<Subject, String>, String> {
    let sessions = live_sessions(connection).await?;
    let runs = live_check_runs(connection).await?;
    Ok(sessions
        .into_iter()
        .map(|(session, project)| (Subject::Session { session }, project))
        .chain(runs)
        .collect())
}

/// Every sandboxed check run not yet sealed, with its project's name.
async fn live_check_runs(connection: &Connection) -> Result<Vec<(Subject, String)>, String> {
    let sandboxed = cockpit_config::ExecutionEnvironment::Sandbox.as_str();
    let runs = cockpit_verification::unsealed_runs_in(connection, sandboxed)
        .await
        .map_err(|error| format!("the unsealed check runs could not be read: {error}"))?;
    Ok(runs
        .into_iter()
        .map(|run| {
            let subject = Subject::Check {
                run: run.run_id,
                check: run.check,
            };
            (subject, run.project)
        })
        .collect())
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

fn subject_view(subject: &Subject) -> NetworkSubjectView {
    match subject {
        Subject::Session { session } => NetworkSubjectView::Session {
            id: session.clone(),
        },
        Subject::Check { run, check } => NetworkSubjectView::Check {
            run: run.clone(),
            check: check.clone(),
        },
        Subject::Containers { project } => NetworkSubjectView::Containers {
            project: project.clone(),
        },
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
        subject: subject_view(&request.subject),
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
