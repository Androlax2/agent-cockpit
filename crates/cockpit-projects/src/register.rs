//! Registering a project: its root must be a plain Git work tree whose metadata it holds.

use std::path::{Path, PathBuf};

use cockpit_turso::Database;

use crate::git::arg;
use crate::store::{self, GitDirIdentity};
use crate::{Checkout, CheckoutKind, Error, Git, Project, ProjectName, blocking};

/// Registers the Git work tree at `root` as a project named `name` (default: the directory's
/// name) and records it as the project's main checkout, incarnation 1.
///
/// Refuses a relative or missing path, a subdirectory of a work tree, a work tree whose Git
/// metadata lives outside `<root>/.git` (a worktree, a submodule or a separate git dir), and a
/// root inside or around another project's checkout.
pub async fn register_project(
    database: &Database,
    git: &Git,
    root: &Path,
    name: Option<&str>,
) -> Result<(Project, Checkout), Error> {
    let invalid = |reason: &str| Error::InvalidPath {
        path: root.to_path_buf(),
        reason: reason.to_owned(),
    };
    if !root.is_absolute() {
        return Err(invalid("a project root is an absolute path"));
    }
    let requested = root.to_path_buf();
    let git = git.clone();
    let root = blocking(move || validate_root(&git, &requested)).await?;
    store::path_text(&root)?;
    let name = match name {
        Some(name) => ProjectName::new(name)?,
        None => ProjectName::new(
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )?,
    };
    let marked = root.clone();
    let identity = blocking(move || {
        GitDirIdentity::create(&marked).map_err(|reason| Error::InvalidPath {
            path: marked,
            reason,
        })
    })
    .await?;
    record(database, &root, &name, &identity)
        .await
        .map_err(|error| error.after_cleanup(identity.discard(&root)))
}

async fn record(
    database: &Database,
    root: &Path,
    name: &ProjectName,
    identity: &GitDirIdentity,
) -> Result<(Project, Checkout), Error> {
    let write = database.write().await?;
    for (project, path) in store::occupied_paths(&write).await? {
        if root.starts_with(&path) || path.starts_with(root) {
            return Err(Error::Overlaps { project, path });
        }
    }
    let project = store::insert_project(&write, name, root).await?;
    let main =
        store::insert_checkout(&write, &project, &CheckoutKind::Main, root, identity).await?;
    write.commit().await?;
    Ok((project, main))
}

/// The canonical root, or why `requested` cannot be a project root.
fn validate_root(git: &Git, requested: &Path) -> Result<PathBuf, Error> {
    let invalid = |reason: String| Error::InvalidPath {
        path: requested.to_path_buf(),
        reason,
    };
    let root = requested
        .canonicalize()
        .map_err(|error| invalid(format!("cannot be resolved: {error}")))?;
    if !root.is_dir() {
        return Err(invalid("is not a directory".to_owned()));
    }
    let answer = git
        .run(
            &root,
            &[
                arg("rev-parse"),
                arg("--show-toplevel"),
                arg("--absolute-git-dir"),
            ],
        )
        .map_err(|error| invalid(format!("is not a Git work tree: {error}")))?;
    let mut lines = answer.lines();
    let (Some(top_level), Some(git_dir)) = (lines.next(), lines.next()) else {
        return Err(invalid(format!("git rev-parse answered {answer:?}")));
    };
    if Path::new(top_level) != root {
        return Err(invalid(format!(
            "is inside the work tree {top_level}; register that directory instead"
        )));
    }
    if Path::new(git_dir) != root.join(".git") {
        return Err(invalid(format!(
            "its Git metadata is at {git_dir}, outside {}/.git (a worktree, submodule or \
             separate git dir); use a plain clone",
            root.display()
        )));
    }
    Ok(root)
}
