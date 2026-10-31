use std::fmt;

/// Why a messaging request was not carried out: the caller's to correct, not a daemon failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A value of the request is not acceptable; `field` names it.
    InvalidArgument {
        field: &'static str,
        reason: String,
    },
    /// The idempotency key already names a different effect. One key is one logical effect:
    /// reusing it for other content would make a receiver drop the second as a duplicate.
    EffectKeyReused {
        effect_key: String,
    },
    NotFound {
        envelope: i64,
    },
    /// Only an uncertain delivery waits for a decision on what happened.
    NotUncertain {
        state: &'static str,
    },
    /// The feed cannot continue from the cursor; the caller starts again from a snapshot.
    ResnapshotRequired(ResnapshotReason),
}

/// Why a cursor cannot be resumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResnapshotReason {
    /// The cursor belongs to another database epoch: a restore or another installation.
    OtherEpoch,
    /// The changes after the cursor are no longer retained.
    Discarded { retained_after: u64 },
    /// The cursor is past every commit of this database.
    Ahead { newest: u64 },
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::InvalidArgument { field, reason } => write!(formatter, "{field}: {reason}"),
            Refusal::EffectKeyReused { effect_key } => write!(
                formatter,
                "the effect key {effect_key:?} already names a different effect"
            ),
            Refusal::NotFound { envelope } => write!(formatter, "no envelope {envelope}"),
            Refusal::NotUncertain { state } => {
                write!(formatter, "the delivery is {state}, not uncertain")
            }
            Refusal::ResnapshotRequired(ResnapshotReason::OtherEpoch) => {
                formatter.write_str("the cursor belongs to another database epoch")
            }
            Refusal::ResnapshotRequired(ResnapshotReason::Discarded { retained_after }) => write!(
                formatter,
                "changes are retained after revision {retained_after} only"
            ),
            Refusal::ResnapshotRequired(ResnapshotReason::Ahead { newest }) => write!(
                formatter,
                "the cursor is past the newest revision, {newest}"
            ),
        }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    Refused(Refusal),
    /// A stored row breaks this crate's schema rules; `reason` says how.
    InvalidStoredRow {
        table: &'static str,
        reason: String,
    },
    Database(cockpit_turso::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused(refusal) => write!(formatter, "refused: {refusal}"),
            Error::InvalidStoredRow { table, reason } => write!(formatter, "{table}: {reason}"),
            Error::Database(error) => write!(formatter, "{error}"),
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

impl From<Refusal> for Error {
    fn from(refusal: Refusal) -> Self {
        Error::Refused(refusal)
    }
}

impl From<cockpit_turso::Error> for Error {
    fn from(source: cockpit_turso::Error) -> Self {
        Error::Database(source)
    }
}

impl From<cockpit_turso::sql::Error> for Error {
    fn from(source: cockpit_turso::sql::Error) -> Self {
        Error::Database(cockpit_turso::Error::Database(source))
    }
}

pub(crate) fn invalid(field: &'static str, reason: impl Into<String>) -> Error {
    Error::Refused(Refusal::InvalidArgument {
        field,
        reason: reason.into(),
    })
}

pub(crate) fn invalid_row(table: &'static str, reason: impl Into<String>) -> Error {
    Error::InvalidStoredRow {
        table,
        reason: reason.into(),
    }
}
