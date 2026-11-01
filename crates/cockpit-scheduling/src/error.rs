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
    Messaging(cockpit_messaging::Error),
    Database(cockpit_turso::sql::Error),
    /// Admission held an agent turn the pass had counted room for: the two disagree on what
    /// is charged, and the pass rolls back rather than place work admission would not.
    PlanAdmissionMismatch {
        item: cockpit_tasks::ItemId,
        hold: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidStoredRow { table, reason } => write!(formatter, "{table}: {reason}"),
            Error::Tasks(error) => write!(formatter, "tasks: {error}"),
            Error::Workflows(error) => write!(formatter, "workflows: {error}"),
            Error::Admission(error) => write!(formatter, "resource admission: {error}"),
            Error::Authorization(error) => write!(formatter, "authorization: {error}"),
            Error::Messaging(error) => write!(formatter, "messaging: {error}"),
            Error::Database(error) => write!(formatter, "database: {error}"),
            Error::PlanAdmissionMismatch { item, hold } => write!(
                formatter,
                "the pass found room for item {}, but admission holds it: {hold}",
                item.0
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::InvalidStoredRow { .. } | Error::PlanAdmissionMismatch { .. } => None,
            Error::Tasks(source) => Some(source),
            Error::Workflows(source) => Some(source),
            Error::Admission(source) => Some(source),
            Error::Authorization(source) => Some(source),
            Error::Messaging(source) => Some(source),
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

impl From<cockpit_messaging::Error> for Error {
    fn from(error: cockpit_messaging::Error) -> Error {
        Error::Messaging(error)
    }
}

impl From<cockpit_turso::sql::Error> for Error {
    fn from(error: cockpit_turso::sql::Error) -> Error {
        Error::Database(error)
    }
}
