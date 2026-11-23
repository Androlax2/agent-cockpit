//! A project's default branch protection on its forge (#695): `torlek project protect` sets
//! it up, and `torlek doctor --project DIR` reports a branch that lacks it with the command
//! that fixes it. Reading takes the project's API token. Changing the repository's settings
//! takes a right neither project token has (#857), so protect uses the token the owner types
//! for that one call and stores it nowhere (Theo, 2026-11-23). `routes.rs` sends
//! `ProjectProtect` here from the owner's socket alone.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use cockpit_forge_api::{ForgeError, ProtectOutcome, ProtectionPolicy, Token};
use cockpit_projects::Git;
use cockpit_protocol::{Command, ErrorCode, ProtectOutcomeView, Reply, SecretText};
use cockpit_readiness::{Requirement, Status};
use cockpit_turso::Database;

use crate::approvals::{Answer, Failed, job};
use crate::forge_reads::{Reader, internal, refused};
use crate::forges::Adapters;
use crate::routes::{Route, WorkClass};

/// The readiness line's name.
const REQUIREMENT: &str = "protection";

/// Longest `doctor` waits for the forge: the read is three calls to it.
const READ_DEADLINE: Duration = Duration::from_secs(20);

pub(crate) struct Context {
    pub(crate) database: Arc<Database>,
    pub(crate) git: Git,
    pub(crate) environment: cockpit_config::Environment,
    pub(crate) adapters: Arc<Adapters>,
}

/// The route for `ProjectProtect`; `routes.rs` sends only that command here.
pub(crate) fn route(command: Command, context: Context) -> Route {
    let Command::ProjectProtect {
        project,
        required_checks,
        token,
    } = command
    else {
        unreachable!("routes.rs sends only ProjectProtect here: {command:?}");
    };
    Route::Run {
        // It waits on the forge, so it never holds a control worker.
        class: WorkClass::Bulk,
        job: job(protect(context, project, required_checks, token)),
    }
}

async fn protect(
    context: Context,
    project: String,
    required_checks: Vec<String>,
    token: SecretText,
) -> Answer {
    let policy = ProtectionPolicy::new(required_checks).map_err(|error| {
        refused(
            ErrorCode::InvalidArgument,
            error.to_string(),
            format!(
                "name each required check once: `torlek project protect {project} --check NAME`"
            ),
        )
    })?;
    let token = Token::new(token.expose()).map_err(|error| {
        refused(
            ErrorCode::InvalidArgument,
            error.to_string(),
            "paste the token alone, on one line".to_owned(),
        )
    })?;
    let root = project_root(&context.database, &project).await?;
    let protected = tokio::task::spawn_blocking(move || {
        let reader = Reader {
            git: &context.git,
            environment: &context.environment,
            adapters: &context.adapters,
        };
        let what = format!("{project}'s default branch protection");
        reader
            .reach(&project, &root, &what, &token, |forge, access| {
                forge.protect_default_branch(access, &policy)
            })?
            .map(|protected| (project.clone(), protected))
            .map_err(|error| protect_failed(&project, &error))
    })
    .await
    .map_err(|error| internal(&error))??;
    let (project, protected) = protected;
    Ok(Reply::ProjectProtected {
        project,
        branch: protected.branch.as_str().to_owned(),
        outcome: match protected.outcome {
            ProtectOutcome::Created => ProtectOutcomeView::Created,
            ProtectOutcome::Updated => ProtectOutcomeView::Updated,
            ProtectOutcome::Unchanged => ProtectOutcomeView::Unchanged,
        },
    })
}

async fn project_root(database: &Database, project: &str) -> Result<std::path::PathBuf, Failed> {
    let read = database.read().await.map_err(|error| internal(&error))?;
    match cockpit_projects::project_named(&read, project).await {
        Ok(found) => Ok(found.root),
        Err(cockpit_projects::Error::UnknownProject { .. }) => Err(refused(
            ErrorCode::NotFound,
            format!("no project is named {project:?}"),
            "name a project `torlek project list` shows".to_owned(),
        )),
        Err(error) => Err(internal(&error)),
    }
}

fn protect_failed(project: &str, error: &ForgeError) -> Failed {
    let again = format!("`torlek project protect {project} --check NAME` again");
    let action = match error {
        ForgeError::Unsupported { .. } | ForgeError::Withheld { .. } => {
            "protect the default branch in the forge's own settings".to_owned()
        }
        ForgeError::Refused { .. } | ForgeError::NotFound { .. } => format!(
            "give a token that may change {project}'s repository settings (administration \
             write, this repository alone), then {again}"
        ),
        ForgeError::Unavailable { .. } | ForgeError::Malformed { .. } => {
            format!("fix what the message names, then {again}")
        }
    };
    let code = match error {
        ForgeError::NotFound { .. } => ErrorCode::NotFound,
        _ => ErrorCode::Refused,
    };
    refused(
        code,
        format!("{project}'s default branch was not protected: {error}"),
        action,
    )
}

/// `doctor`'s line for `project`'s default branch: available when it takes a change request
/// and a check and refuses force pushes and deletion, otherwise each gap and the command that
/// closes them. Not blocking: sessions work on an unprotected project. Reads the forge with the
/// API token, waiting [`READ_DEADLINE`] at most.
pub(crate) async fn requirement(context: Context, project: String, root: &Path) -> Requirement {
    let root = root.to_owned();
    let named = project.clone();
    let reading = tokio::task::spawn_blocking(move || {
        let reader = Reader {
            git: &context.git,
            environment: &context.environment,
            adapters: &context.adapters,
        };
        reader.read(
            &named,
            &root,
            &format!("{named}'s default branch protection"),
            "`torlek doctor` again",
            |forge, access| forge.default_branch_protection(access),
        )
    });
    let read = match tokio::time::timeout(READ_DEADLINE, reading).await {
        Ok(Ok(read)) => read,
        Ok(Err(error)) => Err(internal(&error)),
        Err(_) => {
            return Requirement::needs(
                REQUIREMENT,
                Status::Unknown,
                format!(
                    "the forge did not answer within {} s",
                    READ_DEADLINE.as_secs()
                ),
                "run `torlek doctor` again",
            );
        }
    };
    let protection = match read {
        Ok(protection) => protection,
        Err(Failed(error)) => {
            let next = error.unblock.map_or_else(
                || "run `torlek doctor` again".to_owned(),
                |unblock| unblock.action,
            );
            return Requirement::needs(REQUIREMENT, Status::Unknown, error.message, next);
        }
    };
    let branch = &protection.branch;
    let gaps = protection.rules.gaps();
    if gaps.is_empty() {
        return Requirement::available(
            REQUIREMENT,
            format!(
                "{branch} takes a change request with {} passing, and refuses force pushes and \
                 deletion",
                protection.rules.required_checks.join(", ")
            ),
        );
    }
    let gaps: Vec<String> = gaps.iter().map(ToString::to_string).collect();
    let gaps = gaps.join(", ");
    let protect =
        format!("`torlek project protect {project} --check NAME`, naming each check CI must pass");
    if protection.has_unread_protection {
        return Requirement::needs(
            REQUIREMENT,
            Status::Unknown,
            format!(
                "{branch}'s rules the API token reads: {gaps}; the forge marks it protected by \
                 rules the token cannot read, such as an older kind of branch protection"
            ),
            protect,
        );
    }
    Requirement::needs(
        REQUIREMENT,
        Status::NeedsAuthorization,
        format!("{branch} is not protected: {gaps}"),
        protect,
    )
}
