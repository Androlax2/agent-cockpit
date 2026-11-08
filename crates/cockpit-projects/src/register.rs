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
/// is wrong and what to do next, in plain words; Git's own words appear only in a failure
/// this crate does not recognise (#1183).
fn validate_root(git: &Git, requested: &Path) -> Result<PathBuf, Error> {
    let invalid = |reason: String| Error::InvalidPath {
        path: requested.to_path_buf(),
        reason,
    };
    let root = requested.canonicalize().map_err(|error| {
        invalid(match error.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
                "does not exist; check the path, then add the folder that holds the project"
                    .to_owned()
            }
            // The kind alone: the OS error text and number say nothing more to the owner.
            kind => format!("cannot be opened ({kind}); fix that, then add it again"),
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
        .map_err(|error| invalid(repository_check_failure(git, &root, error)))?;
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

/// Why `git rev-parse` could not describe the repository at `root`, for the owner. What Git
/// found is decided without reading its message, which follows the daemon's locale: a bare
/// repository answers `--is-bare-repository`, and a folder Git finds no repository at is plain
/// when it has no `.git` entry of its own. Anything else keeps Git's cause without the command
/// or exit code.
fn repository_check_failure(git: &Git, root: &Path, error: Error) -> String {
    const GIT_REPOSITORY_FAILURE: i32 = 128;
    if let Error::Git {
        status: Some(GIT_REPOSITORY_FAILURE),
        ..
    } = &error
    {
        match git.run(root, &[arg("rev-parse"), arg("--is-bare-repository")]) {
            Ok(answer) if answer.trim() == "true" => {
                return "is a bare repository, without files to work on. Clone it, then add \
                        the clone"
                    .to_owned();
            }
            Err(Error::Git {
                status: Some(GIT_REPOSITORY_FAILURE),
                ..
            }) if root.join(".git").symlink_metadata().is_err() => {
                return "is not a Git repository. Torlek gives each agent its own clone of the \
                        project, which needs Git. Run `git init` in it, then add it again"
                    .to_owned();
            }
            _ => {}
        }
    }
    let cause = match &error {
        Error::Git { stderr, status, .. } => match stderr.trim() {
            "" => status.map_or("Git stopped on a signal".to_owned(), |code| {
                format!("Git exited with {code} and said nothing")
            }),
            cause => cause.strip_prefix("fatal: ").unwrap_or(cause).to_owned(),
        },
        other => other.to_string(),
    };
    format!("could not be checked for a Git repository: {cause}. Fix that, then add it again")
}
