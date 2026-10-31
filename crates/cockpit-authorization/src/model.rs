use std::fmt;
use std::str::FromStr;

use crate::Error;

/// A session's id as the sessions capability issued it. Opaque: compared byte for byte, never
/// by pattern, case or prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    pub fn new(id: impl Into<String>) -> Result<SessionId, Error> {
        non_empty(id.into(), "session").map(SessionId)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A task's id as the tasks capability issued it. Opaque, like [`SessionId`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskId(String);

impl TaskId {
    pub fn new(id: impl Into<String>) -> Result<TaskId, Error> {
        non_empty(id.into(), "task").map(TaskId)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn non_empty(id: String, kind: &'static str) -> Result<String, Error> {
    if id.is_empty() {
        return Err(Error::EmptyId { kind });
    }
    Ok(id)
}

/// What a grant allows. The names are the stored and future manifest spelling; only the exact
/// name parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    ReadState,
    ChangeState,
    /// Ask the host to run a command or a guide (`ask run`, `ask guide`); each run still needs
    /// the owner's approval of that exact command.
    RunCommands,
    ReadPaneContents,
    WriteClipboard,
    /// Owner-only actions such as `handoff-install` and `system-install` (report 17). Never
    /// granted to a session.
    HostActions,
}

impl Permission {
    pub const ALL: [Permission; 6] = [
        Permission::ReadState,
        Permission::ChangeState,
        Permission::RunCommands,
        Permission::ReadPaneContents,
        Permission::WriteClipboard,
        Permission::HostActions,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Permission::ReadState => "read-state",
            Permission::ChangeState => "change-state",
            Permission::RunCommands => "run-commands",
            Permission::ReadPaneContents => "read-pane-contents",
            Permission::WriteClipboard => "write-clipboard",
            Permission::HostActions => "host-actions",
        }
    }

    pub fn is_owner_only(self) -> bool {
        self == Permission::HostActions
    }
}

impl FromStr for Permission {
    type Err = Error;

    fn from_str(name: &str) -> Result<Permission, Error> {
        Permission::ALL
            .into_iter()
            .find(|permission| permission.as_str() == name)
            .ok_or_else(|| Error::UnknownPermission {
                name: name.to_string(),
            })
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What a grant covers, and what a request touches. An owner-scope grant covers every scope;
/// a session or task grant covers exactly that session or task, nothing nested or similar.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    Owner,
    Session(SessionId),
    Task(TaskId),
}

/// Who asks, as the daemon authenticated them (kernel peer identity, launch token). Never
/// taken from a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// The human who owns the installation; holds every permission.
    Owner,
    Session(SessionId),
}

/// The authorization state's version: every grant and revocation increments it. A decision
/// made at an older revision may rest on a grant revoked since.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(pub(crate) i64);

impl Revision {
    pub fn value(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GrantId(pub(crate) i64);

impl GrantId {
    pub fn value(self) -> i64 {
        self.0
    }
}

impl fmt::Display for GrantId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// One permission over one scope, held by one session, given by the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub id: GrantId,
    pub holder: SessionId,
    pub permission: Permission,
    pub scope: Scope,
    pub granted_at: Revision,
}

/// The answer to "may this happen", with the revision of the grants it read. Callers record
/// the revision with the effect they apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub revision: Revision,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Allowed(Basis),
    Refused(Refusal),
}

/// Why a request was allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Basis {
    Owner,
    Grant(GrantId),
}

/// Why a request was refused, and what would unblock it. Only the owner can unblock either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The owner granting this permission over this scope would allow the request.
    NoGrant {
        permission: Permission,
        scope: Scope,
    },
    /// Only the owner may do this; no grant allows a session to.
    OwnerOnly { permission: Permission },
}
