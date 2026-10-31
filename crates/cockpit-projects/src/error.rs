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
            Error::InvalidPullRequest { pull_request } => {
                write!(formatter, "pull request {pull_request} is out of range")
            }
            Error::InvalidPath { path, reason } => {
                write!(formatter, "{}: {reason}", path.display())
            }
            Error::NameTaken { name } => {
                write!(formatter, "a project named {name} is already registered")
            }
            Error::Overlaps { project, path } => write!(
                formatter,
                "overlaps project {project}'s checkout {}; projects do not nest",
                path.display()
            ),
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
                "checkout {checkout} at {} was replaced outside Cockpit ({reason}); \
                 it is not the recorded incarnation",
                path.display()
            ),
            Error::Refused { reason } => formatter.write_str(reason),
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
