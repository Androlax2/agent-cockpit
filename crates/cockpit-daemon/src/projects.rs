//! Routes the project and checkout commands to `cockpit-projects` and converts its types to
//! the wire at this boundary. The rules live in that crate; this file only maps.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cockpit_projects::{
    Checkout, CheckoutId, CheckoutKind, CheckoutState, Error, Git, Holder, Kept, Trust,
};
use cockpit_protocol::{
    CheckoutKindView, CheckoutView, Command, ErrorCode, KeptCheckout, KeptReason, Outcome,
    ProjectView, Reply, TrustView,
};
use cockpit_turso::Database;

use crate::routes::{Route, WorkClass, refusal};

/// The route for one of the project and checkout commands; `routes.rs` sends only those here.
pub(crate) fn route(
    command: Command,
    database: Arc<Database>,
    git: &Git,
    environment: &cockpit_config::Environment,
) -> Route {
    let git = git.clone();
    let environment = environment.clone();
    let (class, job): (WorkClass, crate::Job) = match command {
        Command::ProjectList => (
            WorkClass::Control,
            job(async move {
                let read = database.read().await.map_err(Error::from)?;
                let projects = cockpit_projects::projects(&read).await?;
                Ok(Reply::Projects {
                    projects: projects.iter().map(project_view).collect(),
                })
            }),
        ),
        Command::CheckoutList { project } => (
            WorkClass::Control,
            job(async move {
                let project = project
                    .map(cockpit_projects::ProjectName::new)
                    .transpose()?;
                let trusted = trusted_roots(&environment).await?;
                let read = database.read().await.map_err(Error::from)?;
                let checkouts = cockpit_projects::checkouts(&read, project.as_ref()).await?;
                Ok(Reply::Checkouts {
                    checkouts: checkouts
                        .iter()
                        .map(|checkout| checkout_view(checkout, &trusted))
                        .collect(),
                })
            }),
        ),
        Command::CheckoutClaim { checkout, holder } => (
            WorkClass::Control,
            job(async move {
                let holder = Holder::new(holder)?;
                let trusted = trusted_roots(&environment).await?;
                let write = database.write().await.map_err(Error::from)?;
                let claimed =
                    cockpit_projects::claim(&write, CheckoutId(checkout), &holder).await?;
                write.commit().await.map_err(Error::from)?;
                Ok(checkout_reply(&claimed, &trusted))
            }),
        ),
        Command::CheckoutRelease { checkout, holder } => (
            WorkClass::Control,
            job(async move {
                let holder = Holder::new(holder)?;
                let write = database.write().await.map_err(Error::from)?;
                cockpit_projects::release(&write, CheckoutId(checkout), &holder).await?;
                write.commit().await.map_err(Error::from)?;
                Ok(Reply::Released { checkout })
            }),
        ),
        Command::ProjectAdd { path, name } => (
            WorkClass::Bulk,
            job(async move {
                let trusted = trusted_roots(&environment).await?;
                let (project, main) = cockpit_projects::register_project(
                    &database,
                    &git,
                    Path::new(&path),
                    name.as_deref(),
                )
                .await?;
                Ok(Reply::ProjectAdded {
                    project: project_view(&project),
                    main: Box::new(checkout_view(&main, &trusted)),
                })
            }),
        ),
        Command::CheckoutClone { project, branch } => (
            WorkClass::Bulk,
            job(async move {
                let trusted = trusted_roots(&environment).await?;
                let clone =
                    cockpit_projects::create_parallel_clone(&database, &git, &project, &branch)
                        .await?;
                Ok(checkout_reply(&clone, &trusted))
            }),
        ),
        Command::CheckoutReview {
            project,
            pull_request,
        } => (
            WorkClass::Bulk,
            job(async move {
                let trusted = trusted_roots(&environment).await?;
                let clone =
                    cockpit_projects::create_review_clone(&database, &git, &project, pull_request)
                        .await?;
                Ok(checkout_reply(&clone, &trusted))
            }),
        ),
        Command::CheckoutSweep { dry_run } => (
            WorkClass::Bulk,
            job(async move {
                let trusted = trusted_roots(&environment).await?;
                let report = cockpit_projects::sweep(&database, &git, dry_run).await?;
                Ok(Reply::Swept {
                    dry_run,
                    removed: report
                        .removed
                        .iter()
                        .map(|checkout| checkout_view(checkout, &trusted))
                        .collect(),
                    kept: report
                        .kept
                        .iter()
                        .map(|(checkout, kept)| KeptCheckout {
                            checkout: checkout_view(checkout, &trusted),
                            reason: kept_reason(kept),
                        })
                        .collect(),
                })
            }),
        ),
        Command::CheckoutPush { checkout } => (
            WorkClass::Bulk,
            job(async move {
                let pushed = cockpit_projects::push(&database, &git, CheckoutId(checkout)).await?;
                Ok(Reply::Pushed {
                    checkout,
                    branch: pushed.branch,
                    remote_url: pushed.remote_url,
                })
            }),
        ),
        other => {
            tracing::error!(
                ?other,
                "a non-project command was routed to the project handler"
            );
            return Route::Answer(refusal(
                ErrorCode::Internal,
                "the daemon routed this command to the wrong handler",
            ));
        }
    };
    Route::Run { class, job }
}

/// The lane job for a project operation: its result becomes the request's outcome.
fn job(work: impl Future<Output = Result<Reply, Error>> + Send + 'static) -> crate::Job {
    Box::pin(async move {
        match work.await {
            Ok(reply) => Outcome::Ok(reply),
            Err(error) => {
                let code = error_code(&error);
                if code == ErrorCode::Internal {
                    tracing::error!(%error, "a project command failed");
                }
                refusal(code, &error.to_string())
            }
        }
    })
}

fn error_code(error: &Error) -> ErrorCode {
    match error {
        Error::InvalidName { .. }
        | Error::InvalidHolder { .. }
        | Error::InvalidBranch { .. }
        | Error::InvalidPullRequest { .. }
        | Error::InvalidPath { .. } => ErrorCode::InvalidArgument,
        Error::UnknownProject { .. } | Error::UnknownCheckout { .. } => ErrorCode::NotFound,
        Error::NameTaken { .. }
        | Error::Overlaps { .. }
        | Error::PathTaken { .. }
        | Error::Occupied { .. }
        | Error::NotOccupant { .. }
        | Error::BeingRemoved { .. } => ErrorCode::Conflict,
        Error::Replaced { .. } => ErrorCode::StaleRevision,
        Error::Refused { .. } => ErrorCode::Refused,
        _ => ErrorCode::Internal,
    }
}

/// The owner's trusted project paths, read before the command acts: a broken owner config
/// refuses the command instead of answering it with every checkout untrusted, or after it
/// already took effect. Read off the async workers, since it reads files.
async fn trusted_roots(environment: &cockpit_config::Environment) -> Result<Vec<PathBuf>, Error> {
    let environment = environment.clone();
    let configured = tokio::task::spawn_blocking(move || {
        cockpit_config::load(&environment, None, &[])
            .map(|config| cockpit_projects::resolve_trusted_roots(config.trusted_projects.value))
    })
    .await
    .map_err(|error| Error::Blocking(error.to_string()))?;
    configured.map_err(|error| Error::Refused {
        reason: format!("the owner configuration cannot be read: {error}"),
    })
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn project_view(project: &cockpit_projects::Project) -> ProjectView {
    ProjectView {
        name: project.name.to_string(),
        root: text(&project.root),
    }
}

fn checkout_view(checkout: &Checkout, trusted_roots: &[PathBuf]) -> CheckoutView {
    CheckoutView {
        id: checkout.id.0,
        project: checkout.project.to_string(),
        kind: match &checkout.kind {
            CheckoutKind::Main => CheckoutKindView::Main,
            CheckoutKind::Parallel { branch } => CheckoutKindView::Parallel {
                branch: branch.clone(),
            },
            CheckoutKind::Review { pull_request } => CheckoutKindView::Review {
                pull_request: *pull_request,
            },
        },
        path: text(&checkout.path),
        incarnation: checkout.incarnation,
        removing: checkout.state != CheckoutState::Live,
        occupant: checkout
            .occupancy
            .as_ref()
            .map(|occupancy| occupancy.holder.to_string()),
        trust: match cockpit_projects::trust(checkout, trusted_roots) {
            Trust::Trusted => TrustView::Trusted,
            Trust::Inherited { from } => TrustView::Inherited { from: text(&from) },
            Trust::Untrusted => TrustView::Untrusted,
            Trust::NeverForReview => TrustView::NeverForReview,
        },
    }
}

fn checkout_reply(checkout: &Checkout, trusted_roots: &[PathBuf]) -> Reply {
    Reply::Checkout {
        checkout: Box::new(checkout_view(checkout, trusted_roots)),
    }
}

fn kept_reason(kept: &Kept) -> KeptReason {
    match kept {
        Kept::Occupied { holder } => KeptReason::Occupied {
            holder: holder.to_string(),
        },
        Kept::UncommittedChanges => KeptReason::UncommittedChanges,
        Kept::UnpushedCommits => KeptReason::UnpushedCommits,
        Kept::Replaced { reason } => KeptReason::Replaced {
            reason: reason.clone(),
        },
        Kept::Unreadable { reason } => KeptReason::Unreadable {
            reason: reason.clone(),
        },
    }
}
