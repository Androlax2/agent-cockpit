use std::fmt;
use std::path::PathBuf;

use crate::ExecutionId;

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The launcher did not confirm the start within the launch timeout. The execution keeps
    /// its reservation; [`crate::Supervisor::observe`] or a restart's reconciliation decides
    /// whether it started.
    LaunchTimedOut {
        execution: ExecutionId,
    },
    /// The launcher exited without starting the program; `launcher_output` is the start of
    /// what it wrote to the execution's stderr (a terminal execution's went to its terminal).
    LaunchFailed {
        execution: ExecutionId,
        launcher_output: String,
    },
    /// The execution has not confirmed its start, so it has no identity to follow yet.
    NotIdentified {
        execution: ExecutionId,
    },
    /// The execution is not a launch intent still waiting for its launch: it was launched
    /// already, or it ended.
    NotReserved {
        execution: ExecutionId,
    },
    /// No execution has this id.
    UnknownExecution {
        execution: ExecutionId,
    },
    /// A stored row breaks this crate's schema rules; `reason` says how.
    InvalidStoredExecution {
        execution: ExecutionId,
        reason: String,
    },
    /// A host command this crate depends on (`systemctl`, `ps`, `sysctl`) failed or did not
    /// answer in time.
    HostCommand {
        command: String,
        reason: String,
    },
    /// A launch spec's environment variable name that is empty or holds `=`.
    InvalidEnvironment {
        name: String,
    },
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    Database(cockpit_turso::Error),
}

impl Error {
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
            Error::LaunchTimedOut { execution } => write!(
                formatter,
                "the launcher of {execution} did not confirm the start in time; \
                 it keeps its reservation until it is observed"
            ),
            Error::LaunchFailed {
                execution,
                launcher_output,
            } => write!(
                formatter,
                "the launcher of {execution} exited without starting it: {launcher_output:?}"
            ),
            Error::InvalidEnvironment { name } => {
                write!(formatter, "{name:?} is not an environment variable name")
            }
            Error::UnknownExecution { execution } => write!(formatter, "{execution} is unknown"),
            Error::NotReserved { execution } => {
                write!(formatter, "{execution} is not waiting for its launch")
            }
            Error::NotIdentified { execution } => {
                write!(formatter, "{execution} has not confirmed its start yet")
            }
            Error::InvalidStoredExecution { execution, reason } => {
                write!(formatter, "the stored {execution} is invalid: {reason}")
            }
            Error::HostCommand { command, reason } => {
                write!(formatter, "`{command}` failed: {reason}")
            }
            Error::Io {
                action,
                path,
                source,
            } => write!(formatter, "cannot {action} {}: {source}", path.display()),
            Error::Database(error) => write!(formatter, "state database: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::Database(error) => Some(error),
            _ => None,
        }
    }
}

impl From<cockpit_turso::Error> for Error {
    fn from(error: cockpit_turso::Error) -> Error {
        Error::Database(error)
    }
}

impl From<cockpit_turso::sql::Error> for Error {
    fn from(error: cockpit_turso::sql::Error) -> Error {
        Error::Database(cockpit_turso::Error::from(error))
    }
}
