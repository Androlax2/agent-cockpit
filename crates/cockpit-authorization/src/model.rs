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
    /// Read the session list and each session's state, doing-now, open question and latest
    /// report, never pane contents or transcripts (#410).
    ObserveSessions,
    /// Queue a message to another session, delivered at its next prompt (#410).
    MessageSessions,
    /// Ask to start a session; each start needs the owner's approval unless the grant's
    /// [`StartTerms`] say otherwise, and is capped per hour (#410).
    StartSessions,
    /// Owner-only actions such as `handoff-install` and `system-install` (report 17). Never
    /// granted to a session.
    HostActions,
}

impl Permission {
    pub const ALL: [Permission; 9] = [
        Permission::ReadState,
        Permission::ChangeState,
        Permission::RunCommands,
        Permission::ReadPaneContents,
        Permission::WriteClipboard,
        Permission::ObserveSessions,
        Permission::MessageSessions,
        Permission::StartSessions,
        Permission::HostActions,
    ];

    /// The coordinator grants (#410): the one mechanism by which a session acts on other
    /// sessions. Only the owner gives them, one at a time; no other grant implies them.
    pub const COORDINATOR: [Permission; 3] = [
        Permission::ObserveSessions,
        Permission::MessageSessions,
        Permission::StartSessions,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Permission::ReadState => "read-state",
            Permission::ChangeState => "change-state",
            Permission::RunCommands => "run-commands",
            Permission::ReadPaneContents => "read-pane-contents",
            Permission::WriteClipboard => "write-clipboard",
            Permission::ObserveSessions => "observe-sessions",
            Permission::MessageSessions => "message-sessions",
            Permission::StartSessions => "start-sessions",
            Permission::HostActions => "host-actions",
        }
    }

    pub fn is_owner_only(self) -> bool {
        self == Permission::HostActions
    }

    pub fn is_coordinator(self) -> bool {
        Permission::COORDINATOR.contains(&self)
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
    /// A `start-sessions` grant's terms; `None` for every other permission.
    pub start_terms: Option<StartTerms>,
}

/// How a `start-sessions` grant lets its holder start sessions (#410).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StartTerms {
    /// Starts asked or made in any hour, at most; at least 1.
    pub per_hour: u32,
    /// Each start waits for the owner's approval in the `ask run` queue. Without it, the
    /// daemon starts the session at once.
    pub needs_confirmation: bool,
}

impl StartTerms {
    /// The terms [`crate::grant`] gives: every start confirmed, ten an hour.
    pub const DEFAULT: StartTerms = StartTerms {
        per_hour: 10,
        needs_confirmation: true,
    };
}

/// Whether the holder of a `start-sessions` grant may start a session now (#410).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartAllowance {
    /// Allowed under `grant`, read at `revision`; with `needs_confirmation` the start waits
    /// for the owner's approval.
    Allowed {
        grant: GrantId,
        revision: Revision,
        needs_confirmation: bool,
    },
    /// The grant's hourly cap is reached; one more start is allowed from `retry_at`
    /// (seconds since 1970).
    Capped {
        per_hour: u32,
        retry_at: i64,
    },
    Refused(Refusal),
}

/// What a coordinator did with its grant, as the audit records it (#410).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditedAction {
    /// Queued a message to the target session.
    Message,
    /// Asked to start a session, or started one without confirmation; counts against the
    /// grant's hourly cap.
    Start,
    /// The session a start asked for began; the target is the new session.
    Started,
}

impl AuditedAction {
    pub const ALL: [AuditedAction; 3] = [
        AuditedAction::Message,
        AuditedAction::Start,
        AuditedAction::Started,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AuditedAction::Message => "message",
            AuditedAction::Start => "start",
            AuditedAction::Started => "started",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<AuditedAction> {
        AuditedAction::ALL
            .into_iter()
            .find(|action| action.as_str() == name)
    }
}

impl fmt::Display for AuditedAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One use of a coordinator grant, as the caller records it in the transaction of its
/// effect: who acted, under which grant read at which revision, on which session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audit {
    pub actor: SessionId,
    pub permission: Permission,
    pub grant: GrantId,
    pub revision: Revision,
    pub action: AuditedAction,
    /// The session acted on; `None` until a start's session exists.
    pub target: Option<SessionId>,
    /// What the action carried that the audit keeps, such as the approval request's id.
    pub detail: Option<String>,
    /// Seconds since 1970.
    pub at: i64,
}

/// An [`Audit`] as stored, with its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub id: i64,
    pub audit: Audit,
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
