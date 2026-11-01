use std::fmt;

use crate::{GrantId, Permission};

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A session or task id was empty; `kind` says which.
    EmptyId {
        kind: &'static str,
    },
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
    /// A `start-sessions` grant's terms break their rule; `reason` says how.
    InvalidStartTerms {
        reason: &'static str,
    },
    /// Only coordinator grants are audited (#410).
    NotAudited {
        permission: Permission,
    },
    /// A stored audit entry breaks this crate's schema rules; `reason` says how.
    InvalidStoredAudit {
        entry: i64,
        reason: String,
    },
    Database(cockpit_turso::sql::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::EmptyId { kind } => write!(formatter, "a {kind} id is empty"),
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
            Error::InvalidStartTerms { reason } => {
                write!(formatter, "invalid start-sessions terms: {reason}")
            }
            Error::NotAudited { permission } => {
                write!(
                    formatter,
                    "{permission} is not a coordinator grant; it is not audited"
                )
            }
            Error::InvalidStoredAudit { entry, reason } => {
                write!(formatter, "stored audit entry {entry}: {reason}")
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
