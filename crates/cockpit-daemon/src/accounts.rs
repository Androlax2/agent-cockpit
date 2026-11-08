//! `torlek account` (#59, #858): the owner's verbs on login accounts, on the host-only
//! socket. A sign-in runs in the owner's terminal, never in the daemon: the daemon prepares
//! its fresh home, then moves only the login file it left into the account's folder.

use std::path::Path;
use std::sync::Arc;

use cockpit_protocol::{Command, Reply};
use cockpit_sessions::Sessions;

use crate::approvals::{Failed, internal, job};
use crate::routes::{Route, WorkClass};

pub(crate) fn route(command: Command, sessions: Arc<Sessions>) -> Route {
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
            other => unreachable!("routes.rs sends only account verbs here, not {other:?}"),
        }
    });
    Route::Run {
        class: WorkClass::Control,
        job: work,
    }
}

fn utf8(path: &Path) -> Result<String, Failed> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| internal(&format!("{} is not UTF-8", path.display())))
}
