use std::fmt;

use crate::{GrantId, Permission};

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A session or task id was empty.
    EmptyId,
    /// The name is not exactly one permission's name.
    UnknownPermission {
        name: String,
    },
    /// The permission belongs to the owner alone and cannot be granted to a session.
    OwnerOnly {
        permission: Permission,
    },
    /// The grant does not exist or was already revoked.
    GrantNotActive {
        grant: GrantId,
    },
    /// A stored row breaks this crate's schema rules; `reason` says how.
    InvalidStoredGrant {
        grant: GrantId,
        reason: String,
    },
    Database(cockpit_turso::sql::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::EmptyId => formatter.write_str("a session or task id is empty"),
            Error::UnknownPermission { name } => write!(
                formatter,
                "{name:?} is not a permission; permissions are named exactly, one at a time"
            ),
            Error::OwnerOnly { permission } => write!(
                formatter,
                "{permission} is the owner's alone and cannot be granted to a session"
            ),
            Error::GrantNotActive { grant } => {
                write!(
                    formatter,
                    "grant {grant} does not exist or was already revoked"
                )
            }
            Error::InvalidStoredGrant { grant, reason } => {
                write!(formatter, "stored grant {grant}: {reason}")
            }
            Error::Database(source) => write!(formatter, "database: {source}"),
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
