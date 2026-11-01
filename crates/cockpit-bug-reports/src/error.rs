use std::fmt;

use crate::{ReportId, Status};

/// Why a request about bug reports was not carried out: an answer naming what would lift it
/// (report 17), not a failure of the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The request itself is invalid; `field` names the part at fault.
    InvalidRequest {
        field: &'static str,
        reason: String,
    },
    NotFound {
        report: ReportId,
    },
    /// The report is not a draft any more, so it cannot `action`.
    WrongStatus {
        report: ReportId,
        status: Status,
        action: &'static str,
    },
    /// The session opened `limit` drafts today, the most one session may.
    SessionLimit {
        limit: u32,
    },
}

impl Refusal {
    /// What would lift the refusal.
    pub fn next_action(&self) -> String {
        match self {
            Refusal::InvalidRequest { field, .. } => format!("correct {field} and send it again"),
            Refusal::NotFound { .. } => "name a report `cockpit bug-report list` shows".to_owned(),
            Refusal::WrongStatus { .. } => {
                "only a draft changes; open a new report with `cockpit bug-report new`".to_owned()
            }
            Refusal::SessionLimit { .. } => {
                "stop reporting: tell the owner in the conversation; they review the drafts \
                 with `cockpit bug-report list` and can open more with `cockpit bug-report new`"
                    .to_owned()
            }
        }
    }

    /// Whether the owner, not the sender, lifts it.
    pub fn is_owners_to_lift(&self) -> bool {
        matches!(self, Refusal::SessionLimit { .. })
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::InvalidRequest { field, reason } => write!(formatter, "{field}: {reason}"),
            Refusal::NotFound { report } => write!(formatter, "no bug report #{}", report.0),
            Refusal::WrongStatus {
                report,
                status,
                action,
            } => write!(
                formatter,
                "bug report #{} is {}; it cannot {action}",
                report.0,
                status.as_str()
            ),
            Refusal::SessionLimit { limit } => write!(
                formatter,
                "this session opened {limit} bug report drafts today, the most one session may"
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
        reason: String,
    },
    Database(cockpit_turso::sql::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused(refusal) => write!(formatter, "refused: {refusal}"),
            Error::InvalidStoredRow { reason } => write!(formatter, "bug_reports: {reason}"),
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

impl From<Refusal> for Error {
    fn from(refusal: Refusal) -> Self {
        Error::Refused(refusal)
    }
}

impl From<cockpit_turso::sql::Error> for Error {
    fn from(source: cockpit_turso::sql::Error) -> Self {
        Error::Database(source)
    }
}

pub(crate) fn invalid(field: &'static str, reason: impl Into<String>) -> Refusal {
    Refusal::InvalidRequest {
        field,
        reason: reason.into(),
    }
}

pub(crate) fn invalid_row(reason: String) -> Error {
    Error::InvalidStoredRow { reason }
}
