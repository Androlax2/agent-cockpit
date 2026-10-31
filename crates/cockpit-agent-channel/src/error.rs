use std::fmt;

use cockpit_authorization::{Permission, Scope, SessionId};

use crate::process::ProcessError;

/// Why the channel did not carry out an agent's request: a first-class answer the agent can
/// act on (report 17), not a failure of the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No launch waits for this token: it is unknown, or an earlier report spent it.
    UnknownLaunchToken,
    /// The token's launch has no process tree recorded yet.
    LaunchNotStarted { session: SessionId },
    /// The token's launch is real, but the sender is not in its process tree.
    OutsideLaunchTree,
    /// No token, and no bound launch among the sender's ancestors.
    Unidentified,
    /// The sender's process tree could not be read, so it cannot be identified.
    PeerUnverifiable(ProcessError),
    /// `cockpit-authorization` refused the identified session.
    NotAuthorized(cockpit_authorization::Refusal),
    /// The request itself is invalid; `field` names the part at fault.
    InvalidRequest { field: &'static str, reason: String },
}

/// Who can lift a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnblockedBy {
    /// The human who owns the installation.
    Owner,
    /// The agent that asked, by changing its request.
    Agent,
    /// Cockpit itself: a missing feature or a fault to report.
    Cockpit,
}

/// What would lift a refusal, and who can do it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unblock {
    pub action: String,
    pub by: UnblockedBy,
}

impl Refusal {
    pub fn unblock(&self) -> Unblock {
        let (by, action) = match self {
            Refusal::UnknownLaunchToken | Refusal::Unidentified => (
                UnblockedBy::Owner,
                "restart the session from Cockpit so its launch is bound to this process tree"
                    .to_owned(),
            ),
            Refusal::LaunchNotStarted { session } => (
                UnblockedBy::Cockpit,
                format!(
                    "session {} has not finished launching; retry",
                    session.as_str()
                ),
            ),
            Refusal::OutsideLaunchTree => (
                UnblockedBy::Agent,
                "send the report from a process the session launched".to_owned(),
            ),
            Refusal::PeerUnverifiable(ProcessError::Unsupported) => (
                UnblockedBy::Cockpit,
                "agent identity on this platform is not implemented yet".to_owned(),
            ),
            Refusal::PeerUnverifiable(_) => (
                UnblockedBy::Agent,
                "send the report from a process that stays alive until it is answered".to_owned(),
            ),
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
        };
        Unblock { action, by }
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
            Refusal::UnknownLaunchToken => {
                formatter.write_str("the launch token is unknown or already spent")
            }
            Refusal::LaunchNotStarted { session } => write!(
                formatter,
                "session {}'s launch has no process tree yet",
                session.as_str()
            ),
            Refusal::OutsideLaunchTree => formatter
                .write_str("the launch token belongs to a process tree the sender is not part of"),
            Refusal::Unidentified => formatter.write_str(
                "the sender is in no bound session's process tree and sent no launch token",
            ),
            Refusal::PeerUnverifiable(error) => {
                write!(formatter, "the sender cannot be identified: {error}")
            }
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
        }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    Refused(Refusal),
    /// `record_root` found no launch of the session waiting for its process tree.
    NoPendingLaunch {
        session: SessionId,
    },
    /// The system's random source could not make a launch token.
    Randomness(std::io::Error),
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
            Error::NoPendingLaunch { session } => write!(
                formatter,
                "session {} has no launch waiting for its process tree",
                session.as_str()
            ),
            Error::Randomness(error) => write!(formatter, "cannot make a launch token: {error}"),
            Error::InvalidStoredRow { table, reason } => write!(formatter, "{table}: {reason}"),
            Error::Authorization(error) => write!(formatter, "authorization: {error}"),
            Error::Database(error) => write!(formatter, "database: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Randomness(source) => Some(source),
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
