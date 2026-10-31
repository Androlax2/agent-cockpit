use std::fmt;

use cockpit_authorization::{Permission, Scope};

use crate::{ItemId, Status};

/// Why a request about tasks was not carried out: an answer naming what would lift it and
/// who can (report 17), not a failure of the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// `cockpit-authorization` refused the session.
    NotAuthorized(cockpit_authorization::Refusal),
    /// Only the owner does this; no grant lets a session.
    OwnerOnly { action: &'static str },
    /// The request itself is invalid; `field` names the part at fault.
    InvalidRequest { field: &'static str, reason: String },
    /// No such item, or one the session may not see.
    NotFound { item: ItemId },
    /// The item is not in a status the action applies to.
    WrongStatus {
        item: ItemId,
        status: Status,
        action: &'static str,
    },
    /// It still waits on these items.
    Waiting { item: ItemId, on: Vec<ItemId> },
    /// The change was asked on an intent revision that is no longer current.
    StaleRevision { current: i64 },
    /// The owner declined this title in this project before.
    Declined { project: String },
    /// The filing project already has `open` items in `project`'s list, the cap.
    CapReached {
        project: String,
        open: u64,
        cap: u32,
    },
    /// No blocker project is configured.
    NoBlockerProject,
    /// The session works in no registered project, so it has no list of its own.
    NoProject,
    /// Open items wait on it.
    WaitedOn { item: ItemId, by: Vec<ItemId> },
    /// The wait would make the item wait on itself.
    Cycle { item: ItemId, on: ItemId },
    /// The session may change only items it does.
    NotDoing { item: ItemId },
    /// The image is too large, there are too many, or it is not an image.
    InvalidImage { reason: String },
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
                format!("ask the owner to {action} with `cockpit todo` on the host"),
            ),
            Refusal::InvalidRequest { field, .. } => (
                UnblockedBy::Agent,
                format!("correct {field} and send it again"),
            ),
            Refusal::NotFound { .. } => (
                UnblockedBy::Agent,
                "name an item of your project, or one you filed".to_owned(),
            ),
            Refusal::WrongStatus { .. } => (
                UnblockedBy::Agent,
                "read the item again with `cockpit todo show`".to_owned(),
            ),
            Refusal::Waiting { on, .. } => (
                UnblockedBy::Owner,
                format!(
                    "finish {} first, or remove the wait with `cockpit todo unwait`",
                    item_list(on)
                ),
            ),
            Refusal::StaleRevision { current } => (
                UnblockedBy::Agent,
                format!("read the item again and edit intent revision {current}"),
            ),
            Refusal::Declined { .. } => (
                UnblockedBy::Agent,
                "do not file it again; the owner decided".to_owned(),
            ),
            Refusal::CapReached { project, .. } => (
                UnblockedBy::Owner,
                format!("the owner finishes, declines or deletes an item filed in {project}"),
            ),
            Refusal::NoBlockerProject => (
                UnblockedBy::Owner,
                "set `tasks.blocker_project` in the owner configuration".to_owned(),
            ),
            Refusal::NoProject => (
                UnblockedBy::Owner,
                "register the project the session works in with `cockpit project add`".to_owned(),
            ),
            Refusal::WaitedOn { by, .. } => (
                UnblockedBy::Owner,
                format!("remove the waits of {} first", item_list(by)),
            ),
            Refusal::Cycle { .. } => (
                UnblockedBy::Agent,
                "name an item that does not wait on this one".to_owned(),
            ),
            Refusal::NotDoing { .. } => (
                UnblockedBy::Agent,
                "start the item first with `cockpit todo start`".to_owned(),
            ),
            Refusal::InvalidImage { .. } => (
                UnblockedBy::Owner,
                "attach a PNG, JPEG, GIF or WebP image of at most 4 MiB".to_owned(),
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

fn item_list(items: &[ItemId]) -> String {
    items
        .iter()
        .map(|item| format!("#{}", item.0))
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
            Refusal::NotFound { item } => write!(formatter, "no item #{}", item.0),
            Refusal::WrongStatus {
                item,
                status,
                action,
            } => write!(
                formatter,
                "item #{} is {}; it cannot {action}",
                item.0,
                status.as_str()
            ),
            Refusal::Waiting { item, on } => write!(
                formatter,
                "item #{} still waits on {}",
                item.0,
                item_list(on)
            ),
            Refusal::StaleRevision { current } => write!(
                formatter,
                "the item changed since it was read; its intent revision is now {current}"
            ),
            Refusal::Declined { project } => write!(
                formatter,
                "the owner declined this title in {project} before"
            ),
            Refusal::CapReached { project, open, cap } => write!(
                formatter,
                "this project already has {open} open items in {project} (the cap is {cap})"
            ),
            Refusal::NoBlockerProject => {
                formatter.write_str("no project is configured for blocker reports")
            }
            Refusal::NoProject => formatter.write_str("the session works in no registered project"),
            Refusal::WaitedOn { item, by } => {
                write!(formatter, "{} wait on item #{}", item_list(by), item.0)
            }
            Refusal::Cycle { item, on } => write!(
                formatter,
                "item #{} cannot wait on #{}: #{} already waits on it",
                item.0, on.0, on.0
            ),
            Refusal::NotDoing { item } => {
                write!(formatter, "this session is not doing item #{}", item.0)
            }
            Refusal::InvalidImage { reason } => write!(formatter, "image: {reason}"),
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
