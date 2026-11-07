use std::fmt;

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A value handed in breaks its rule; `field` names it.
    Invalid {
        field: &'static str,
        reason: String,
    },
    /// A reading taken before the one already recorded for the account: a late result must
    /// not replace a newer one.
    OlderReading {
        observed_at: i64,
        recorded_at: i64,
    },
    /// A stored row breaks this crate's schema rules; `reason` says how.
    InvalidStoredRow {
        reason: String,
    },
    Database(cockpit_turso::sql::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid { field, reason } => write!(formatter, "{field}: {reason}"),
            Error::OlderReading {
                observed_at,
                recorded_at,
            } => write!(
                formatter,
                "a quota reading taken at {observed_at} is older than the one recorded at \
                 {recorded_at}"
            ),
            Error::InvalidStoredRow { reason } => write!(formatter, "usage_quota: {reason}"),
            Error::Database(error) => write!(formatter, "database: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Database(source) => Some(source),
            _ => None,
        }
    }
}

impl From<cockpit_turso::sql::Error> for Error {
    fn from(source: cockpit_turso::sql::Error) -> Self {
        Error::Database(source)
    }
}

pub(crate) fn invalid(field: &'static str, reason: impl Into<String>) -> Error {
    Error::Invalid {
        field,
        reason: reason.into(),
    }
}

pub(crate) fn invalid_row(reason: impl Into<String>) -> Error {
    Error::InvalidStoredRow {
        reason: reason.into(),
    }
}
