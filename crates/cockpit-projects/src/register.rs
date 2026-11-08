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
            return Err(Error::Overlaps {
                root: root.to_path_buf(),
                project,
                path,
            });
        }
    }
    let project = store::insert_project(&write, name, root).await?;
    let main =
        store::insert_checkout(&write, &project, &CheckoutKind::Main, root, identity).await?;
    write.commit().await?;
    Ok((project, main))
}

/// The canonical root, or why `requested` cannot be a project root. Every refusal says what
/// is wrong and what to do next; Git's own words appear only for a failure other than "not a
/// repository" (#1183).
fn validate_root(git: &Git, requested: &Path) -> Result<PathBuf, Error> {
    let invalid = |reason: String| Error::InvalidPath {
        path: requested.to_path_buf(),
        reason,
    };
    let root = requested.canonicalize().map_err(|error| {
        invalid(if error.kind() == std::io::ErrorKind::NotFound {
            "does not exist; check the path, then add the folder that holds the project".to_owned()
        } else {
            format!("cannot be opened ({error}); fix that, then add it again")
        })
    })?;
    if !root.is_dir() {
        return Err(invalid(
            "is not a folder; add the folder that holds the project instead".to_owned(),
        ));
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
        .map_err(|error| invalid(repository_check_failure(error)))?;
    let mut lines = answer.lines();
    let (Some(top_level), Some(git_dir)) = (lines.next(), lines.next()) else {
        return Err(invalid(format!(
            "Git described this repository in an unexpected way ({}); check that `git` is a \
             standard Git, then add it again",
            answer.trim()
        )));
    };
    if Path::new(top_level) != root {
        return Err(invalid(format!(
            "is a folder inside the Git repository at {top_level}; a project is a whole \
             repository. Add {top_level} instead"
        )));
    }
    if Path::new(git_dir) != root.join(".git") {
        return Err(invalid(format!(
            "keeps its Git metadata at {git_dir}, outside its own .git folder, as a worktree, a \
             submodule or a repository with a separate git dir does. Torlek clones each agent's \
             copy from a plain repository: add the repository that owns this folder, or clone it \
             on its own and add the clone"
        )));
    }
    Ok(root)
}

/// Why checking for a repository at the root failed, for the owner: a plain folder is told to
/// run `git init`; any other failure keeps Git's cause without the command or exit code.
fn repository_check_failure(error: Error) -> String {
    match error {
        Error::Git { stderr, status, .. } => {
            let cause = stderr.trim();
            if status == Some(128) && cause.contains("not a git repository") {
                return "is not a Git repository. Torlek gives each agent its own clone of the \
                        project, which needs Git. Run `git init` in it, then add it again"
                    .to_owned();
            }
            let cause = if cause.is_empty() {
                status.map_or("Git stopped on a signal".to_owned(), |code| {
                    format!("Git exited with {code} and said nothing")
                })
            } else {
                cause.strip_prefix("fatal: ").unwrap_or(cause).to_owned()
            };
            format!(
                "could not be checked for a Git repository: {cause}. Fix that, then add it again"
            )
        }
        other => format!(
            "could not be checked for a Git repository: {other}. Fix that, then add it again"
        ),
    }
}
