use std::fmt;
use std::path::PathBuf;

use crate::{CheckoutId, Holder, ProjectName};

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    InvalidName {
        name: String,
        reason: String,
    },
    InvalidHolder {
        holder: String,
    },
    InvalidBranch {
        branch: String,
    },
    /// A push-branch pattern the owner gave that names no branch or reaches beyond branches.
    InvalidPushBranch {
        pattern: String,
        reason: String,
    },
    /// Beyond what the store holds: SQLite integers are signed.
    InvalidPullRequest {
        pull_request: u64,
    },
    /// The path cannot be a project root or checkout; `reason` says why.
    InvalidPath {
        path: PathBuf,
        reason: String,
    },
    NameTaken {
        name: ProjectName,
    },
    /// The root is inside, or contains, a checkout of an existing project.
    Overlaps {
        root: PathBuf,
        project: ProjectName,
        path: PathBuf,
    },
    UnknownProject {
        name: String,
    },
    UnknownCheckout {
        checkout: CheckoutId,
    },
    /// Something already exists where the checkout would be created.
    PathTaken {
        path: PathBuf,
    },
    Occupied {
        checkout: CheckoutId,
        holder: Holder,
    },
    NotOccupant {
        checkout: CheckoutId,
        holder: Holder,
    },
    /// The sweep is deleting it; it no longer accepts work.
    BeingRemoved {
        checkout: CheckoutId,
    },
    /// The working copy at the checkout's path is not the incarnation Cockpit recorded.
    Replaced {
        checkout: CheckoutId,
        path: PathBuf,
        reason: String,
    },
    /// A host-Git safety rule refused the operation; `reason` says which.
    Refused {
        reason: String,
    },
    /// An agent's push of `commit` to `branch` brings new content to the CI `paths`
    /// (`.github/workflows`, `.gitlab-ci.yml`; #857): only the owner's approval of that commit
    /// lets it go.
    WorkflowChange {
        checkout: CheckoutId,
        branch: String,
        commit: String,
        paths: Vec<String>,
    },
    /// An owner decision no longer names the current observed target.
    StalePrune {
        reason: String,
    },
    /// Git ran and failed.
    Git {
        arguments: Vec<String>,
        status: Option<i32>,
        stderr: String,
    },
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    /// A stored row breaks this crate's schema rules.
    InvalidStoredRow {
        reason: String,
    },
    Database(cockpit_turso::Error),
    Sql(cockpit_turso::sql::Error),
    /// A blocking Git or filesystem step panicked.
    Blocking(String),
    /// `cause` stopped the operation, and undoing what it had done failed too; something is
    /// left on disk, so the daemon answers `internal`.
    CleanupFailed {
        cause: Box<Error>,
        cleanup: Box<Error>,
    },
}

impl Error {
    /// Whether the database failed, here or in a cleanup after another error: the same call
    /// may succeed later, unlike a refusal or what Git said about a checkout.
    pub fn is_database_failure(&self) -> bool {
        match self {
            Error::Database(_) | Error::Sql(_) => true,
            Error::CleanupFailed { cause, cleanup } => {
                cause.is_database_failure() || cleanup.is_database_failure()
            }
            _ => false,
        }
    }

    /// `self` when `cleanup` succeeded, both otherwise.
    pub(crate) fn after_cleanup(self, cleanup: Result<(), Error>) -> Error {
        match cleanup {
            Ok(()) => self,
            Err(cleanup) => Error::CleanupFailed {
                cause: Box::new(self),
                cleanup: Box::new(cleanup),
            },
        }
    }

    pub(crate) fn io(
        action: &'static str,
        path: impl Into<PathBuf>,
    ) -> impl FnOnce(std::io::Error) -> Error {
        let path = path.into();
        move |source| Error::Io {
            action,
            path,
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidName { name, reason } => {
                write!(formatter, "{name:?} is not a valid project name: {reason}")
            }
            Error::InvalidHolder { holder } => write!(
                formatter,
                "{holder:?} is not a holder id: 1 to 256 characters, no control characters"
            ),
            Error::InvalidBranch { branch } => {
                write!(formatter, "{branch:?} is not a valid branch name")
            }
            Error::InvalidPushBranch { pattern, reason } => {
                write!(
                    formatter,
                    "{pattern:?} is not a push-branch pattern: {reason}"
                )
            }
            Error::InvalidPullRequest { pull_request } => {
                write!(formatter, "pull request {pull_request} is out of range")
            }
            Error::InvalidPath { path, reason } => {
                write!(formatter, "{}: {reason}", path.display())
            }
            Error::NameTaken { name } => {
                write!(formatter, "a project named {name} is already registered")
            }
            Error::Overlaps {
                root,
                project,
                path,
            } => {
                if root == path {
                    return write!(
                        formatter,
                        "{}: is already project {project}'s checkout; a project is added once",
                        root.display()
                    );
                }
                let relation = if root.starts_with(path) {
                    "is inside"
                } else {
                    "contains"
                };
                write!(
                    formatter,
                    "{}: {relation} project {project}'s checkout {}; projects do not nest. Add \
                     a folder outside every registered project, or work in project {project}",
                    root.display(),
                    path.display()
                )
            }
            Error::UnknownProject { name } => write!(formatter, "no project is named {name:?}"),
            Error::UnknownCheckout { checkout } => {
                write!(formatter, "no checkout has id {checkout}")
            }
            Error::PathTaken { path } => write!(
                formatter,
                "{} already exists; refusing to create a checkout over it",
                path.display()
            ),
            Error::Occupied { checkout, holder } => {
                write!(formatter, "checkout {checkout} is occupied by {holder}")
            }
            Error::NotOccupant { checkout, holder } => {
                write!(formatter, "{holder} does not occupy checkout {checkout}")
            }
            Error::BeingRemoved { checkout } => {
                write!(
                    formatter,
                    "checkout {checkout} is being removed by the sweep"
                )
            }
            Error::Replaced {
                checkout,
                path,
                reason,
            } => write!(
                formatter,
                "checkout {checkout} at {} was replaced outside Torlek ({reason}); \
                 it is not the recorded incarnation",
                path.display()
            ),
            Error::Refused { reason } | Error::StalePrune { reason } => formatter.write_str(reason),
            Error::WorkflowChange {
                checkout,
                branch,
                commit,
                paths,
            } => write!(
                formatter,
                "pushing {commit} to {branch} changes what CI runs ({}): its content there is \
                 neither the remote branch's nor the default branch's. Only the owner pushes it: \
                 `torlek checkout push {} --branch {branch} --commit {commit}`, a fast-forward \
                 (a rewrite of the branch the owner pushes by hand); a branch that only predates \
                 the default branch's change goes once rebased onto it",
                paths.join(", "),
                checkout.0
            ),
            Error::Git {
                arguments,
                status,
                stderr,
            } => {
                let status = status.map_or("a signal".to_owned(), |code| code.to_string());
                write!(
                    formatter,
                    "git {} failed with {status}: {}",
                    arguments.join(" "),
                    stderr.trim()
                )
            }
            Error::Io {
                action,
                path,
                source,
            } => write!(formatter, "could not {action} {}: {source}", path.display()),
            Error::InvalidStoredRow { reason } => write!(formatter, "stored checkout: {reason}"),
            Error::CleanupFailed { cause, cleanup } => {
                write!(formatter, "{cause}; cleaning up also failed: {cleanup}")
            }
            Error::Database(source) => write!(formatter, "database: {source}"),
            Error::Sql(source) => write!(formatter, "database: {source}"),
            Error::Blocking(reason) => write!(formatter, "a blocking step failed: {reason}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::CleanupFailed { cause, .. } => Some(cause.as_ref()),
            Error::Database(source) => Some(source),
            Error::Sql(source) => Some(source),
            _ => None,
        }
    }
}

impl From<cockpit_turso::Error> for Error {
    fn from(source: cockpit_turso::Error) -> Error {
        Error::Database(source)
    }
}

impl From<cockpit_turso::sql::Error> for Error {
    fn from(source: cockpit_turso::sql::Error) -> Error {
        Error::Sql(source)
    }
}
