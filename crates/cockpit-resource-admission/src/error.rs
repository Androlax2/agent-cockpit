use std::fmt;
use std::path::PathBuf;

use crate::ReservationId;

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The request itself is malformed; `reason` says how.
    InvalidRequest {
        reason: String,
    },
    UnknownReservation {
        reservation: ReservationId,
    },
    /// The execution has no process group yet, so there is nothing to act on.
    NotStarted {
        execution: cockpit_processes::ExecutionId,
    },
    /// The operation needs the reservation in another state.
    WrongState {
        reservation: ReservationId,
        state: &'static str,
        needed: &'static str,
    },
    /// A stored row breaks this crate's schema rules; `reason` says how.
    InvalidStoredReservation {
        reservation: ReservationId,
        reason: String,
    },
    /// A stored setting or sum breaks this crate's schema rules; `reason` says how.
    InvalidStoredState {
        reason: String,
    },
    /// A host command this crate depends on (`systemctl`, `ps`, `sysctl`) failed or did not
    /// answer in time.
    HostCommand {
        command: String,
        reason: String,
    },
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    Database(cockpit_turso::Error),
    Processes(cockpit_processes::Error),
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
            Error::InvalidRequest { reason } => write!(formatter, "invalid request: {reason}"),
            Error::UnknownReservation { reservation } => {
                write!(formatter, "no {reservation} exists")
            }
            Error::NotStarted { execution } => write!(formatter, "{execution} has not started"),
            Error::WrongState {
                reservation,
                state,
                needed,
            } => write!(
                formatter,
                "{reservation} is {state}; this needs it {needed}"
            ),
            Error::InvalidStoredReservation {
                reservation,
                reason,
            } => write!(formatter, "{reservation} is stored invalid: {reason}"),
            Error::InvalidStoredState { reason } => {
                write!(formatter, "the stored resource state is invalid: {reason}")
            }
            Error::HostCommand { command, reason } => write!(formatter, "`{command}`: {reason}"),
            Error::Io {
                action,
                path,
                source,
            } => write!(formatter, "cannot {action} {}: {source}", path.display()),
            Error::Database(error) => write!(formatter, "state database: {error}"),
            Error::Processes(error) => write!(formatter, "executions: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::Database(error) => Some(error),
            Error::Processes(error) => Some(error),
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
        Error::Database(error.into())
    }
}

impl From<cockpit_processes::Error> for Error {
    fn from(error: cockpit_processes::Error) -> Error {
        Error::Processes(error)
    }
}
