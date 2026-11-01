use std::fmt;

use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_provider_api::Capability;
use cockpit_tasks::ItemId;

use crate::{RunId, RunStatus, StepStatus};

/// Why a request about workflows was not carried out: an answer naming what would lift it and
/// who can (report 17), not a failure of the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// `cockpit-authorization` refused the session.
    NotAuthorized(cockpit_authorization::Refusal),
    /// Only the owner does this; no grant lets a session.
    OwnerOnly { action: &'static str },
    /// The request itself is invalid; `field` names the part at fault.
    InvalidRequest { field: &'static str, reason: String },
    /// No preset has this name.
    UnknownPreset {
        name: String,
        known: Vec<&'static str>,
    },
    /// No such run, or one the session may not see.
    NotFound { run: RunId },
    /// No such item, or one the session may not see.
    ItemNotFound { item: ItemId },
    /// The item is not open work (`todo` or `doing`), so no workflow starts on it.
    ItemNotOpen { item: ItemId, status: &'static str },
    /// The item already follows this running workflow.
    AlreadyRunning { item: ItemId, run: RunId },
    /// The run is delivered or cancelled.
    NotRunning { run: RunId, status: RunStatus },
    /// The run waits on a blocker's action first.
    Blocked { run: RunId },
    /// The current step is not in a status the action applies to, or is the other doer's.
    WrongStep {
        run: RunId,
        position: u32,
        status: StepStatus,
        action: &'static str,
    },
    /// The session's provider lacks what the step declares.
    MissingCapabilities {
        step: &'static str,
        missing: Vec<Capability>,
    },
    /// The session is already doing a step of another running workflow.
    AlreadyDoing { run: RunId, position: u32 },
    /// The session is not doing the run's current step.
    NotDoing { run: RunId },
    /// The step's requirements have neither passed nor been waived.
    RequirementsUnmet {
        step: &'static str,
        missing: Vec<&'static str>,
    },
}

/// Who can lift a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnblockedBy {
    Owner,
    Agent,
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
            Refusal::OwnerOnly { action } => (
                UnblockedBy::Owner,
                format!("ask the owner to {action} with `cockpit workflow` on the host"),
            ),
            Refusal::InvalidRequest { field, .. } => (
                UnblockedBy::Agent,
                format!("correct {field} and send it again"),
            ),
            Refusal::UnknownPreset { known, .. } => (
                UnblockedBy::Owner,
                format!("name one of the presets: {}", known.join(", ")),
            ),
            Refusal::NotFound { .. } => (
                UnblockedBy::Agent,
                "name a workflow of your project with `cockpit workflow list`".to_owned(),
            ),
            Refusal::ItemNotFound { .. } => (
                UnblockedBy::Owner,
                "name an item of `cockpit todo list`".to_owned(),
            ),
            Refusal::ItemNotOpen { .. } => (
                UnblockedBy::Owner,
                "reopen or approve the item first with `cockpit todo`".to_owned(),
            ),
            Refusal::AlreadyRunning { run, .. } => (
                UnblockedBy::Owner,
                format!(
                    "continue workflow {}, or cancel it with `cockpit workflow cancel`",
                    run.0
                ),
            ),
            Refusal::NotRunning { .. } => (
                UnblockedBy::Owner,
                "start a new workflow on the item".to_owned(),
            ),
            Refusal::Blocked { run } => (
                UnblockedBy::Owner,
                format!(
                    "do what the blocker asks; then the owner, or the session doing the step, \
                     runs `cockpit workflow unblock {}`",
                    run.0
                ),
            ),
            Refusal::WrongStep { run, .. } => (
                UnblockedBy::Agent,
                format!(
                    "read the workflow again with `cockpit workflow show {}`",
                    run.0
                ),
            ),
            Refusal::MissingCapabilities { .. } => (
                UnblockedBy::Owner,
                "give the step to a session whose provider reports them".to_owned(),
            ),
            Refusal::AlreadyDoing { run, .. } => (
                UnblockedBy::Agent,
                format!("complete or release your step of workflow {} first", run.0),
            ),
            Refusal::NotDoing { run } => (
                UnblockedBy::Agent,
                format!(
                    "begin the step first with `cockpit workflow begin {}`",
                    run.0
                ),
            ),
            Refusal::RequirementsUnmet { .. } => (
                UnblockedBy::Agent,
                "record passing evidence for them, or ask the owner to waive them".to_owned(),
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

fn names<T: AsRef<str>>(values: &[T]) -> String {
    values
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join(", ")
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
            Refusal::OwnerOnly { action } => write!(formatter, "only the owner can {action}"),
            Refusal::InvalidRequest { field, reason } => write!(formatter, "{field}: {reason}"),
            Refusal::UnknownPreset { name, .. } => write!(formatter, "no workflow preset {name:?}"),
            Refusal::NotFound { run } => write!(formatter, "no workflow {}", run.0),
            Refusal::ItemNotFound { item } => write!(formatter, "no item #{}", item.0),
            Refusal::ItemNotOpen { item, status } => {
                write!(formatter, "item #{} is {status}", item.0)
            }
            Refusal::AlreadyRunning { item, run } => write!(
                formatter,
                "item #{} already follows workflow {}",
                item.0, run.0
            ),
            Refusal::NotRunning { run, status } => {
                write!(formatter, "workflow {} is {}", run.0, status.as_str())
            }
            Refusal::Blocked { run } => write!(formatter, "workflow {} is blocked", run.0),
            Refusal::WrongStep {
                run,
                position,
                status,
                action,
            } => write!(
                formatter,
                "step {position} of workflow {} is {}; it cannot {action}",
                run.0,
                status.as_str()
            ),
            Refusal::MissingCapabilities { step, missing } => write!(
                formatter,
                "step {step} needs {}, which the session's provider does not report",
                names(
                    &missing
                        .iter()
                        .map(|capability| capability.as_str())
                        .collect::<Vec<_>>()
                )
            ),
            Refusal::AlreadyDoing { run, position } => write!(
                formatter,
                "this session is doing step {position} of workflow {}",
                run.0
            ),
            Refusal::NotDoing { run } => write!(
                formatter,
                "this session is not doing the current step of workflow {}",
                run.0
            ),
            Refusal::RequirementsUnmet { step, missing } => write!(
                formatter,
                "step {step} still needs passing evidence for {}",
                names(missing)
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
    Tasks(cockpit_tasks::Error),
    Authorization(cockpit_authorization::Error),
    Database(cockpit_turso::sql::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused(refusal) => write!(formatter, "refused: {refusal}"),
            Error::InvalidStoredRow { table, reason } => write!(formatter, "{table}: {reason}"),
            Error::Tasks(error) => write!(formatter, "tasks: {error}"),
            Error::Authorization(error) => write!(formatter, "authorization: {error}"),
            Error::Database(error) => write!(formatter, "database: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Tasks(source) => Some(source),
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

pub(crate) fn invalid_row(table: &'static str, reason: impl Into<String>) -> Error {
    Error::InvalidStoredRow {
        table,
        reason: reason.into(),
    }
}

pub(crate) fn session_of(
    table: &'static str,
    text: Option<String>,
) -> Result<Option<SessionId>, Error> {
    text.map(|text| SessionId::new(text).map_err(|error| invalid_row(table, error.to_string())))
        .transpose()
}
