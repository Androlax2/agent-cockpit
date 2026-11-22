//! A project's open issues on its forge, shown beside its todos (#60): read through the forge
//! contract with the project's API token, for reads only (#857 as amended by Theo on
//! 2026-11-22). A project with no API token has no issues to show. `routes.rs` sends
//! `TaskIssues` here from the owner's socket alone.

use std::path::Path;
use std::sync::Arc;

use cockpit_forge_api::{Issue, IssueFilter, IssueState, ListLimit};
use cockpit_projects::Git;
use cockpit_protocol::{Command, ErrorCode, IssueView, Reply};
use cockpit_turso::Database;

use crate::approvals::{Answer, Failed, job};
use crate::forge_reads::{Reader, internal, refused};
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

/// Blocking: see [`Reader::read`].
fn list_open_issues(context: &Context, project: &str, root: &Path) -> Result<Vec<Issue>, Failed> {
    let reader = Reader {
        git: &context.git,
        environment: &context.environment,
        adapters: &context.adapters,
    };
    let filter = IssueFilter {
        state: Some(IssueState::Open),
        labels: Vec::new(),
        limit: ListLimit::new(LISTED).expect("the listing size is within the forge limit"),
    };
    reader.read(
        project,
        root,
        &format!("{project}'s open issues"),
        |forge, access| forge.list_issues(access, &filter),
    )
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
