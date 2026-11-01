use std::fmt;

/// What a scheduling call could not do. A capability it calls fails loud through it: a pass
/// that cannot take the work it decided rolls back whole.
#[derive(Debug)]
pub enum Error {
    /// A stored row breaks this crate's schema rules; `reason` says how.
    InvalidStoredRow {
        table: &'static str,
        reason: String,
    },
    Tasks(cockpit_tasks::Error),
    Workflows(cockpit_workflows::Error),
    Admission(cockpit_resource_admission::Error),
    Authorization(cockpit_authorization::Error),
    Database(cockpit_turso::sql::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidStoredRow { table, reason } => write!(formatter, "{table}: {reason}"),
            Error::Tasks(error) => write!(formatter, "tasks: {error}"),
            Error::Workflows(error) => write!(formatter, "workflows: {error}"),
            Error::Admission(error) => write!(formatter, "resource admission: {error}"),
            Error::Authorization(error) => write!(formatter, "authorization: {error}"),
            Error::Database(error) => write!(formatter, "database: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::InvalidStoredRow { .. } => None,
            Error::Tasks(source) => Some(source),
            Error::Workflows(source) => Some(source),
            Error::Admission(source) => Some(source),
            Error::Authorization(source) => Some(source),
            Error::Database(source) => Some(source),
        }
    }
}

impl From<cockpit_tasks::Error> for Error {
    fn from(error: cockpit_tasks::Error) -> Error {
        Error::Tasks(error)
    }
}

impl From<cockpit_workflows::Error> for Error {
    fn from(error: cockpit_workflows::Error) -> Error {
        Error::Workflows(error)
    }
}

impl From<cockpit_resource_admission::Error> for Error {
    fn from(error: cockpit_resource_admission::Error) -> Error {
        Error::Admission(error)
    }
}

impl From<cockpit_authorization::Error> for Error {
    fn from(error: cockpit_authorization::Error) -> Error {
        Error::Authorization(error)
    }
}

impl From<cockpit_turso::sql::Error> for Error {
    fn from(error: cockpit_turso::sql::Error) -> Error {
        Error::Database(error)
    }
}
