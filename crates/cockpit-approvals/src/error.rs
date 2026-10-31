use std::fmt;

use cockpit_authorization::{Permission, Scope};

/// Why a request about an approval or a guide was not carried out: a first-class answer
/// naming what would lift it and who can (report 17), not a failure of the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// `cockpit-authorization` refused the asking session.
    NotAuthorized(cockpit_authorization::Refusal),
    /// The request itself is invalid; `field` names the part at fault.
    InvalidRequest { field: &'static str, reason: String },
    /// No such request, or it belongs to another session: an agent learns nothing about
    /// other sessions' requests.
    NotFound { kind: &'static str, id: i64 },
    /// The decision was made on a revision that is no longer the request's current one: the
    /// command or directory changed since the owner read it (report 04, walkthrough 3).
    StaleRevision { current: i64 },
    /// The digest the decision names is not the current revision's.
    DigestMismatch,
    /// Nobody waited for the request any more, so it can no longer be decided.
    Expired,
    /// The request was decided already; a decision is taken once.
    AlreadyDecided { state: &'static str },
    /// Input goes to a running command only.
    NotRunning,
    /// The answer names a guide step that is not the one shown now.
    StaleStep { current: usize },
}

/// Who can lift a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnblockedBy {
    Owner,
    Agent,
    Cockpit,
}

impl Refusal {
    /// What would lift the refusal, and who can do it.
    pub fn unblock(&self) -> (UnblockedBy, String) {
        match self {
            Refusal::NotAuthorized(cockpit_authorization::Refusal::NoGrant {
                permission,
                scope,
            }) => (UnblockedBy::Owner, grant_action(*permission, scope)),
            Refusal::NotAuthorized(cockpit_authorization::Refusal::OwnerOnly { permission }) => (
                UnblockedBy::Owner,
                format!("only the owner can do this ({permission}); run it on the host"),
            ),
            Refusal::InvalidRequest { field, .. } => (
                UnblockedBy::Agent,
                format!("correct {field} and send it again"),
            ),
            Refusal::NotFound { kind, .. } => (
                UnblockedBy::Agent,
                format!("name a {kind} this session asked for"),
            ),
            Refusal::StaleRevision { current } => (
                UnblockedBy::Owner,
                format!("read the request again and decide on revision {current}"),
            ),
            Refusal::DigestMismatch => (
                UnblockedBy::Owner,
                "read the request again and decide on the digest it shows".to_owned(),
            ),
            Refusal::Expired => (
                UnblockedBy::Agent,
                "ask again if the command is still needed".to_owned(),
            ),
            Refusal::AlreadyDecided { .. } => (
                UnblockedBy::Owner,
                "nothing to do: the request was decided".to_owned(),
            ),
            Refusal::NotRunning => (
                UnblockedBy::Owner,
                "send input only while the command runs".to_owned(),
            ),
            Refusal::StaleStep { current } => (
                UnblockedBy::Owner,
                format!("answer the step shown now, step {}", current + 1),
            ),
        }
    }
}

fn grant_action(permission: Permission, scope: &Scope) -> String {
    let scope = match scope {
        Scope::Owner => "everything".to_owned(),
        Scope::Session(session) => format!("session {}", session.as_str()),
        Scope::Task(task) => format!("task {}", task.as_str()),
    };
    format!("the owner grants {permission} over {scope}")
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NotAuthorized(cockpit_authorization::Refusal::NoGrant {
                permission, ..
            }) => write!(
                formatter,
                "the session holds no {permission} grant for this"
            ),
            Refusal::NotAuthorized(cockpit_authorization::Refusal::OwnerOnly { permission }) => {
                write!(formatter, "{permission} is the owner's alone")
            }
            Refusal::InvalidRequest { field, reason } => write!(formatter, "{field}: {reason}"),
            Refusal::NotFound { kind, id } => write!(formatter, "no {kind} {id}"),
            Refusal::StaleRevision { current } => write!(
                formatter,
                "the request changed since it was read; its revision is now {current}"
            ),
            Refusal::DigestMismatch => {
                formatter.write_str("the digest is not the request's current one")
            }
            Refusal::Expired => {
                formatter.write_str("the request expired: nobody waited for its answer")
            }
            Refusal::AlreadyDecided { state } => {
                write!(formatter, "the request was decided already; it is {state}")
            }
            Refusal::NotRunning => formatter.write_str("the command is not running"),
            Refusal::StaleStep { current } => write!(
                formatter,
                "the guide is at step {}, not the one answered",
                current + 1
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
    Authorization(cockpit_authorization::Error),
    Database(cockpit_turso::sql::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused(refusal) => write!(formatter, "refused: {refusal}"),
            Error::InvalidStoredRow { table, reason } => write!(formatter, "{table}: {reason}"),
            Error::Authorization(error) => write!(formatter, "authorization: {error}"),
            Error::Database(error) => write!(formatter, "database: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Authorization(source) => Some(source),
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

impl From<cockpit_authorization::Error> for Error {
    fn from(source: cockpit_authorization::Error) -> Self {
        Error::Authorization(source)
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
