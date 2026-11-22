//! A project's open issues on its forge, shown beside its todos (#60): read through the forge
//! contract with the project's API token, for reads only (#857 as amended by Theo on
//! 2026-11-22). A project with no API token has no issues to show. `routes.rs` sends
//! `TaskIssues` here from the owner's socket alone.

use std::path::Path;
use std::sync::Arc;

use cockpit_forge_api::{
    Access, Forge, ForgeError, Issue, IssueFilter, IssueState, ListLimit, Token, resolve,
};
use cockpit_projects::Git;
use cockpit_protocol::{Command, ErrorCode, IssueView, ProtocolError, Reply, Unblock, UnblockedBy};
use cockpit_turso::Database;

use crate::approvals::{Answer, Failed, job};
use crate::forges::Adapters;
use crate::routes::{Route, WorkClass};

/// The most issues one listing shows, newest first.
const LISTED: usize = 50;

pub(crate) struct Context {
    pub(crate) database: Arc<Database>,
    pub(crate) git: Git,
    pub(crate) environment: cockpit_config::Environment,
    pub(crate) adapters: Arc<Adapters>,
}

/// The route for `TaskIssues`; `routes.rs` sends only that command here.
pub(crate) fn route(command: Command, context: Context) -> Route {
    let Command::TaskIssues { project } = command else {
        unreachable!("routes.rs sends only TaskIssues here: {command:?}");
    };
    Route::Run {
        // It waits on the forge, so it never holds a control worker.
        class: WorkClass::Bulk,
        job: job(open_issues(context, project)),
    }
}

async fn open_issues(context: Context, project: String) -> Answer {
    let root = {
        let read = context
            .database
            .read()
            .await
            .map_err(|error| internal(&error))?;
        match cockpit_projects::project_named(&read, &project).await {
            Ok(found) => found.root,
            Err(cockpit_projects::Error::UnknownProject { .. }) => {
                return Err(refused(
                    ErrorCode::NotFound,
                    format!("no project is named {project:?}"),
                    "name a project `torlek project list` shows".to_owned(),
                ));
            }
            Err(error) => return Err(internal(&error)),
        }
    };
    let listed = tokio::task::spawn_blocking(move || {
        list_open_issues(&context, &project, &root).map(|issues| (project, issues))
    })
    .await
    .map_err(|error| internal(&error))??;
    let (project, issues) = listed;
    Ok(Reply::Issues {
        project,
        issues: issues.into_iter().map(issue_view).collect(),
    })
}

/// Blocking: it reads the token file, asks Git for the origin and runs the forge's program.
fn list_open_issues(context: &Context, project: &str, root: &Path) -> Result<Vec<Issue>, Failed> {
    let token = api_token(&context.environment, project)?;
    let origin = context
        .git
        .origin_url(root)
        .map_err(|error| internal(&error))?
        .ok_or_else(|| {
            refused(
                ErrorCode::Refused,
                format!("{project} has no `origin` remote, so it has no forge to read issues from"),
                format!("add the repository as `origin` in {}", root.display()),
            )
        })?;
    context
        .adapters
        .read(&context.environment, |adapters| {
            list_through(adapters, project, &origin, &token)
        })
        .map_err(|error| internal(&error))?
}

fn list_through(
    adapters: &[&dyn Forge],
    project: &str,
    origin: &str,
    token: &Token,
) -> Result<Vec<Issue>, Failed> {
    // The origin itself is not repeated: one naming a credential is refused for holding it.
    let (forge, repository) = resolve(adapters, origin).map_err(|unresolved| {
        refused(
            ErrorCode::Refused,
            format!("{project}'s origin has no forge to read issues from: {unresolved}"),
            "point `origin` at the project's https repository URL".to_owned(),
        )
    })?;
    let filter = IssueFilter {
        state: Some(IssueState::Open),
        labels: Vec::new(),
        limit: ListLimit::new(LISTED).expect("the listing size is within the forge limit"),
    };
    let access = Access {
        repository: &repository,
        token,
    };
    forge
        .list_issues(&access, &filter)
        .map_err(|error| forge_failed(project, &error))
}

/// The project's API token: the one a daemon read may use (never its other token).
fn api_token(environment: &cockpit_config::Environment, project: &str) -> Result<Token, Failed> {
    let set_it = format!("`torlek project token set {project} --api`");
    let path = cockpit_config::github_api_token_file(environment, project)
        .ok_or_else(|| internal(&"the daemon's environment names no home, so no token folder"))?;
    let stored = cockpit_config::read_github_token(&path)
        .map_err(|error| {
            refused(
                ErrorCode::Refused,
                format!("{project}'s API token is unusable: {error}"),
                set_it.clone(),
            )
        })?
        .ok_or_else(|| {
            refused(
                ErrorCode::Refused,
                format!("{project} has no API token, so its issues are not read"),
                set_it.clone(),
            )
        })?;
    // The file's rule is stricter than the contract's, so a stored token always converts.
    Token::new(stored.expose()).map_err(|error| internal(&error))
}

fn forge_failed(project: &str, error: &ForgeError) -> Failed {
    let (code, action) = match error {
        ForgeError::NotFound { .. } => (
            ErrorCode::NotFound,
            format!("check that {project}'s API token may read the repository"),
        ),
        ForgeError::Refused { .. } => (
            ErrorCode::Refused,
            format!("check {project}'s API token: `torlek project token show {project} --api`"),
        ),
        ForgeError::Unsupported { .. }
        | ForgeError::Withheld { .. }
        | ForgeError::Unavailable { .. }
        | ForgeError::Malformed { .. } => (
            ErrorCode::Refused,
            format!("fix what the message names, then `torlek todo issues {project}` again"),
        ),
    };
    refused(
        code,
        format!("{project}'s issues were not read: {error}"),
        action,
    )
}

fn refused(code: ErrorCode, message: String, action: String) -> Failed {
    Failed(ProtocolError {
        code,
        message,
        unblock: Some(Unblock {
            action,
            by: UnblockedBy::Owner,
        }),
    })
}

fn internal(error: &dyn std::fmt::Display) -> Failed {
    tracing::error!(%error, "reading a project's issues failed");
    Failed(ProtocolError::new(
        ErrorCode::Internal,
        "the daemon failed to read the project's issues",
    ))
}

fn issue_view(issue: Issue) -> IssueView {
    IssueView {
        id: issue.id.as_str().to_owned(),
        title: issue.title,
        labels: issue.labels,
        author: issue.author,
        url: issue.url,
    }
}
