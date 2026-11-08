//! `torlek account` (#59, #858): the owner's verbs on login accounts, on the host-only
//! socket. A sign-in runs in the owner's terminal, never in the daemon: the daemon prepares
//! its fresh home, then moves only the login file it left into the account's folder. A grant
//! lets one project's sessions use that login, until the owner revokes it.

use std::path::Path;
use std::sync::Arc;

use cockpit_protocol::{
    Command, ErrorCode, LoginGrantView, ProtocolError, Reply, Unblock, UnblockedBy,
};
use cockpit_sandbox::{LoginGrant, LoginGrantError, LoginGrantRefusal};
use cockpit_sessions::Sessions;
use cockpit_turso::Database;

use crate::approvals::{Failed, internal, job, now};
use crate::network::{decision_view, owner_client};
use crate::routes::{Route, WorkClass};

impl From<LoginGrantError> for Failed {
    fn from(error: LoginGrantError) -> Failed {
        let LoginGrantError::Refused(refusal) = &error else {
            return internal(&error);
        };
        let (code, action) = match refusal {
            LoginGrantRefusal::NotFound { .. } => (
                ErrorCode::NotFound,
                "name a grant `torlek account grants` shows".to_owned(),
            ),
            LoginGrantRefusal::AlreadyGranted { .. } | LoginGrantRefusal::AlreadyRevoked => (
                ErrorCode::Conflict,
                "nothing to do: it was decided".to_owned(),
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

pub(crate) fn route(command: Command, sessions: Arc<Sessions>, database: Arc<Database>) -> Route {
    let work = job(async move {
        match command {
            Command::AccountSignIn { provider, account } => {
                let plan = sessions.begin_sign_in(&provider, &account).await?;
                Ok(Reply::AccountSignInPlan {
                    program: utf8(&plan.program)?,
                    arguments: plan.arguments,
                    home: utf8(&plan.home)?,
                    steps: plan.steps,
                })
            }
            Command::AccountSignedIn { provider, account } => {
                let login = sessions.finish_sign_in(&provider, &account).await?;
                tracing::info!(%provider, "the owner signed a login in");
                Ok(Reply::AccountSignedIn {
                    provider,
                    account,
                    login: utf8(&login)?,
                })
            }
            Command::AccountSignInDiscard { provider, account } => {
                sessions.discard_sign_in(&provider, &account).await?;
                Ok(Reply::AccountSignInDiscarded { provider, account })
            }
            Command::AccountGrant {
                project,
                provider,
                account,
                through,
            } => {
                let grant = sessions
                    .grant_login(&project, &provider, &account, through.map(owner_client))
                    .await
                    .map_err(|error| match error {
                        cockpit_sessions::Error::LoginGrants(error) => Failed::from(error),
                        other => Failed::from(other),
                    })?;
                tracing::info!(grant = grant.id, %project, %provider, "the owner granted a login");
                Ok(grant_reply(&grant))
            }
            Command::AccountGrants { project } => {
                let read = database.read().await?;
                let grants = cockpit_sandbox::login_grants(&read, project.as_deref()).await?;
                Ok(Reply::AccountGrants {
                    grants: grants.iter().map(grant_view).collect(),
                })
            }
            Command::AccountRevoke { grant, through } => {
                let write = database.write().await?;
                let revoked = cockpit_sandbox::revoke_login_grant(
                    &write,
                    grant,
                    through.map(owner_client),
                    now(),
                )
                .await?;
                write.commit().await?;
                tracing::info!(grant, "the owner revoked a login grant");
                Ok(grant_reply(&revoked))
            }
            other => unreachable!("routes.rs sends only account verbs here, not {other:?}"),
        }
    });
    Route::Run {
        class: WorkClass::Control,
        job: work,
    }
}

fn grant_reply(grant: &LoginGrant) -> Reply {
    Reply::AccountGrant {
        grant: Box::new(grant_view(grant)),
    }
}

fn grant_view(grant: &LoginGrant) -> LoginGrantView {
    LoginGrantView {
        id: grant.id,
        project: grant.project.clone(),
        provider: grant.provider.clone(),
        account: grant.account.clone(),
        granted: decision_view(grant.granted),
        revoked: grant.revoked.map(decision_view),
    }
}

fn utf8(path: &Path) -> Result<String, Failed> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| internal(&format!("{} is not UTF-8", path.display())))
}
