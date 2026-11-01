#![doc = include_str!("../README.md")]

mod clones;
mod error;
mod git;
mod model;
mod push;
mod register;
mod source;
mod store;
mod sweep;

use std::path::{Path, PathBuf};

pub use clones::{create_parallel_clone, create_review_clone};
pub use error::Error;
pub use git::{Git, SAFE_CONFIG};
pub use model::{
    Checkout, CheckoutId, CheckoutKind, CheckoutState, Holder, Occupancy, Project, ProjectId,
    ProjectName, Trust,
};
pub use push::{Pushed, push};
pub use register::register_project;
pub use source::SourceState;
pub use store::{MIGRATIONS, checkouts, claim, project_named, projects, release};
pub use sweep::{Kept, SweepReport, sweep};

/// Where a project's clones live: `<parent of the root>/.parallel/<project>/`, beside the
/// project, as the current `cockpit` script lays out `~/IdeaProjects/.parallel`.
pub(crate) fn clones_dir(project_root: &Path, project: &ProjectName) -> Result<PathBuf, Error> {
    let parent = project_root.parent().ok_or_else(|| Error::InvalidPath {
        path: project_root.to_path_buf(),
        reason: "a project root has a parent directory to hold its clones".to_owned(),
    })?;
    Ok(parent.join(".parallel").join(project.as_str()))
}

/// The owner's trusted project paths (`projects.trusted`) as [`trust`] compares them: each as
/// configured and, when it resolves, with symbolic links resolved. Reads the filesystem.
pub fn resolve_trusted_roots(configured: Vec<PathBuf>) -> Vec<PathBuf> {
    let resolved: Vec<PathBuf> = configured
        .iter()
        .filter_map(|trusted| trusted.canonicalize().ok())
        .collect();
    configured.into_iter().chain(resolved).collect()
}

/// Whose trust covers `checkout`, given trusted roots from [`resolve_trusted_roots`]. A
/// parallel clone inherits its project's trust; a review clone never has any.
pub fn trust(checkout: &Checkout, trusted_roots: &[PathBuf]) -> Trust {
    let is_trusted = |path: &Path| trusted_roots.iter().any(|trusted| trusted == path);
    match &checkout.kind {
        CheckoutKind::Review { .. } => Trust::NeverForReview,
        _ if is_trusted(&checkout.path) => Trust::Trusted,
        CheckoutKind::Parallel { .. } if is_trusted(&checkout.project_root) => Trust::Inherited {
            from: checkout.project_root.clone(),
        },
        _ => Trust::Untrusted,
    }
}

/// Runs a blocking Git or filesystem step off the async executor and waits for it; a started
/// step always runs to its end (report 03).
pub(crate) async fn blocking<T: Send + 'static>(
    step: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(step)
        .await
        .map_err(|error| Error::Blocking(error.to_string()))?
}
