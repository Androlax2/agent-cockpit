//! An agent's push of its own checkout (`torlek push`, #155): under the `push-branch` grant
//! every launch gives the session over itself, the daemon pushes with the project's token,
//! which the agent never holds. The checkout comes from the session's record, never from the
//! request or the sender's working directory.

use std::sync::Arc;

use cockpit_agent_channel::{ProcessError, ProcessIdentity};
use cockpit_authorization::{
    Actor, Audit, AuditedAction, Basis, GrantId, Outcome as Decided, Permission, Revision, Scope,
};
use cockpit_projects::{AgentBranches, Checkout, CheckoutKind, Force, Git, PushRules, Pusher};
use cockpit_protocol::{Command, ErrorCode, ProtocolError, Reply, Unblock, UnblockedBy};
use cockpit_turso::Database;

use crate::approvals::{Answer, Failed, Sender, job, now};
use crate::routes::{Route, WorkClass};

/// The route for `AgentPush`; `routes.rs` sends only that here, from the agent socket.
pub(crate) fn route(
    command: Command,
    connector: Result<ProcessIdentity, ProcessError>,
    database: Arc<Database>,
    git: Git,
    environment: cockpit_config::Environment,
) -> Route {
    let Command::AgentPush {
        launch_token,
        force,
    } = command
    else {
        tracing::error!(
            ?command,
            "a command other than a push was routed to the agent push"
        );
        return Route::Run {
            class: WorkClass::Bulk,
            job: job(async {
                Err(Failed(ProtocolError::new(
                    ErrorCode::Internal,
                    "the daemon routed this command to the wrong handler",
                )))
            }),
        };
    };
    Route::Run {
        class: WorkClass::Bulk,
        job: job(async move {
            let sender = Sender::read(connector).await;
            let session = crate::coordination::identified(&database, &sender, launch_token).await?;
            push(&database, &git, environment, session, force).await
        }),
    }
}

/// What the push is checked against, read in one transaction before it starts: a revoke
/// landing after this read does not stop a push already under way.
struct Pushing {
    grant: GrantId,
    revision: Revision,
    checkout: Checkout,
    launch: Option<String>,
    branches: AgentBranches,
    bound_hosts: Vec<String>,
}

async fn push(
    database: &Database,
    git: &Git,
    environment: cockpit_config::Environment,
    session: cockpit_authorization::SessionId,
    force: bool,
) -> Answer {
    let pushing = read_pushing(database, &session).await?;
    let force = match (force, &pushing.launch) {
        (false, _) => Force::Never,
        (true, Some(launch)) => Force::Lease {
            own_branch: launch.clone(),
        },
        (true, None) => {
            return Err(refused(
                "this checkout has no launch branch, so no push from it is forced".to_owned(),
                "push without `--force`".to_owned(),
                UnblockedBy::Agent,
            ));
        }
    };
    let project = pushing.checkout.project.as_str().to_owned();
    let credential = crate::projects::push_credential(environment, &project).await;
    let rules = PushRules {
        hosts: crate::forges::push_hosts(&pushing.bound_hosts),
        credential,
        force,
        pusher: Pusher::Agent {
            branches: pushing.branches,
        },
    };
    let pushed = cockpit_projects::push(database, git, pushing.checkout.id, rules)
        .await
        .map_err(|error| push_failed(&session, error))?;
    let audit = Audit {
        actor: session,
        permission: Permission::PushBranch,
        grant: pushing.grant,
        revision: pushing.revision,
        action: AuditedAction::Pushed,
        target: None,
        detail: Some(format!(
            "{} to {} on {}{}",
            pushed.commit,
            pushed.branch,
            pushed.remote_url,
            if pushed.forced { ", forced" } else { "" }
        )),
        at: now(),
    };
    // The push went, so its reply says so; a lost audit is the daemon's fault to report, not
    // a failure the agent would retry into a second push.
    if let Err(error) = record_pushed(database, &audit).await {
        tracing::error!(
            session = audit.actor.as_str(),
            detail = audit.detail.as_deref(),
            %error,
            "an agent's push went but its audit entry could not be written"
        );
    }
    Ok(Reply::Pushed {
        checkout: pushing.checkout.id.0,
        branch: pushed.branch,
        remote_url: pushed.remote_url,
    })
}

async fn record_pushed(database: &Database, audit: &Audit) -> Result<(), String> {
    let write = database.write().await.map_err(|error| error.to_string())?;
    cockpit_authorization::record_audit(&write, audit)
        .await
        .map_err(|error| error.to_string())?;
    write.commit().await.map_err(|error| error.to_string())
}

async fn read_pushing(
    database: &Database,
    session: &cockpit_authorization::SessionId,
) -> Result<Pushing, Failed> {
    let read = database.read().await?;
    let decision = cockpit_authorization::evaluate(
        &read,
        &Actor::Session(session.clone()),
        Permission::PushBranch,
        &Scope::Session(session.clone()),
    )
    .await?;
    let grant = match decision.outcome {
        Decided::Allowed(Basis::Grant(grant)) => grant,
        Decided::Allowed(Basis::Owner) => unreachable!("a session is never allowed as the owner"),
        Decided::Refused(refusal) => {
            tracing::info!(
                session = session.as_str(),
                ?refusal,
                "an agent's push was refused: no push-branch grant"
            );
            return Err(Failed(ProtocolError {
                code: ErrorCode::NotAuthorized,
                message: "this session holds no `push-branch` grant over itself, so the daemon \
                          pushes nothing for it"
                    .to_owned(),
                unblock: Some(Unblock {
                    action: "ask the owner to push the branch".to_owned(),
                    by: UnblockedBy::Owner,
                }),
            }));
        }
    };
    let recorded = cockpit_sessions::session(
        &read,
        &cockpit_sessions::SessionId::parse(session.as_str())?,
    )
    .await?;
    let checkout = cockpit_projects::checkouts(&read, None)
        .await
        .map_err(project_failed)?
        .into_iter()
        .find(|checkout| checkout.id == recorded.checkout)
        .ok_or_else(|| {
            Failed(ProtocolError::new(
                ErrorCode::NotFound,
                "this session's checkout is removed, so there is nothing to push",
            ))
        })?;
    let launch = match &checkout.kind {
        CheckoutKind::Parallel { branch } => Some(branch.clone()),
        CheckoutKind::Main => None,
        CheckoutKind::Review { .. } => {
            return Err(refused(
                "a review checkout holds someone else's pull request head; the daemon pushes \
                 nothing from it for an agent"
                    .to_owned(),
                "ask the owner to push it".to_owned(),
                UnblockedBy::Owner,
            ));
        }
    };
    let project = checkout.project.as_str();
    let patterns = cockpit_projects::push_branches(&read, project)
        .await
        .map_err(project_failed)?;
    let bound_hosts = cockpit_sandbox::push_hosts(&read, project).await?;
    Ok(Pushing {
        grant,
        revision: decision.revision,
        launch: launch.clone(),
        branches: AgentBranches { launch, patterns },
        checkout,
        bound_hosts,
    })
}

/// A push the checks refused, logged with the session that asked, as the wire tells it.
fn push_failed(
    session: &cockpit_authorization::SessionId,
    error: cockpit_projects::Error,
) -> Failed {
    let code = crate::projects::error_code(&error);
    if code == ErrorCode::Internal {
        tracing::error!(session = session.as_str(), %error, "an agent's push failed");
    } else {
        tracing::info!(session = session.as_str(), %error, "an agent's push was refused");
    }
    let unblock = match &error {
        cockpit_projects::Error::WorkflowChange { .. } => Some(Unblock {
            action: "ask the owner to push that commit, with the command the message names"
                .to_owned(),
            by: UnblockedBy::Owner,
        }),
        _ => None,
    };
    Failed(ProtocolError {
        code,
        message: error.to_string(),
        unblock,
    })
}

fn project_failed(error: cockpit_projects::Error) -> Failed {
    let code = crate::projects::error_code(&error);
    if code == ErrorCode::Internal {
        tracing::error!(%error, "reading an agent's push rules failed");
    }
    Failed(ProtocolError::new(code, error.to_string()))
}

fn refused(message: String, action: String, by: UnblockedBy) -> Failed {
    Failed(ProtocolError {
        code: ErrorCode::Refused,
        message,
        unblock: Some(Unblock { action, by }),
    })
}
