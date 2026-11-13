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
    /// The `run-commands` grant the request was asked under no longer covers its session (it
    /// ended, failed or lost the grant, even if granted again since), so the request can no
    /// longer be approved, edited or answered (#285).
    AskerMayNotAsk,
    /// The request was decided already; a decision is taken once.
    AlreadyDecided { state: &'static str },
    /// Input goes to a running command only.
    NotRunning,
    /// The command's terminal may show what is typed, so the text could land in its output
    /// (#1160); `why` says what it does. `request` is the run, for the next step it names.
    InputWouldShow { request: i64, why: String },
    /// The password names a sudo ask of the run that no longer waits (#1374): sudo ended, or
    /// the ask has its answer.
    NoSudoAsk { request: i64, ask: u64 },
    /// The sudo or helper behind the ask changed before the owner's answer (#1374): `why`.
    SudoStoppedAsking { request: i64, ask: u64, why: String },
    /// The secret could not be fingerprinted to be redacted from the run's output (#1386), so
    /// it was not handed on: `why`.
    SecretUnprotected { request: i64, why: String },
    /// Only a command running under this daemon can be stopped (#1301); `why` says what the
    /// request is instead.
    NotStoppable { why: String },
    /// The stop was sent, but the command's processes were not observed gone (#1301).
    StopUnconfirmed { request: i64 },
    /// Only a finished command run stores output to purge; `what` says what the request is.
    NoStoredOutput { what: &'static str },
    /// The answer names a guide step that is not the one shown now.
    StaleStep { current: usize },
    /// The approval came within the grace after the request appeared or changed: it may be a
    /// keystroke meant for something else (#156).
    TooSoon { wait_seconds: i64 },
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
            Refusal::AskerMayNotAsk => (
                UnblockedBy::Owner,
                "nothing to do: its session reads it as expired".to_owned(),
            ),
            Refusal::AlreadyDecided { .. } => (
                UnblockedBy::Owner,
                "nothing to do: the request was decided".to_owned(),
            ),
            Refusal::NotRunning => (
                UnblockedBy::Owner,
                "send input only while the command runs".to_owned(),
            ),
            Refusal::InputWouldShow { request, .. } => (
                UnblockedBy::Owner,
                format!(
                    "stop the command with `torlek approval stop {request}` (x in the TUI), or \
                     send a visible answer on purpose with `torlek approval input {request} \
                     --shown` (Tab in the TUI); type a secret only at a prompt that hides it"
                ),
            ),
            Refusal::NoSudoAsk { request, .. } | Refusal::SudoStoppedAsking { request, .. } => (
                UnblockedBy::Owner,
                format!(
                    "nothing to send: read the request again with `torlek approval show \
                     {request}` and answer the sudo ask it shows, if any"
                ),
            ),
            Refusal::SecretUnprotected { request, .. } => (
                UnblockedBy::Owner,
                format!(
                    "send it again while request {request} still waits; if it fails again, \
                     the daemon cannot read /dev/urandom"
                ),
            ),
            Refusal::NotStoppable { .. } => (
                UnblockedBy::Owner,
                "nothing to stop from Torlek: decline a request that still asks, or end a command \
                 that outlived a daemon restart on the host"
                    .to_owned(),
            ),
            Refusal::StopUnconfirmed { request } => (
                UnblockedBy::Owner,
                format!(
                    "read the request again with `torlek approval show {request}`; if it still \
                     runs, end its processes on the host"
                ),
            ),
            Refusal::NoStoredOutput { .. } => (
                UnblockedBy::Owner,
                "nothing to purge: purge a command run once it has finished".to_owned(),
            ),
            Refusal::StaleStep { current } => (
                UnblockedBy::Owner,
                format!("answer the step shown now, step {}", current + 1),
            ),
            Refusal::TooSoon { wait_seconds } => (
                UnblockedBy::Owner,
                format!("read the request, then approve it again in {wait_seconds} s"),
            ),
        }
    }
}

fn grant_action(permission: Permission, scope: &Scope) -> String {
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
            Refusal::AskerMayNotAsk => formatter.write_str(
                "the session that asked may no longer ask the owner: it ended, failed or lost \
                 its run-commands grant",
            ),
            Refusal::AlreadyDecided { state } => {
                write!(formatter, "the request was decided already; it is {state}")
            }
            Refusal::NotRunning => formatter.write_str("the command is not running"),
            Refusal::InputWouldShow { request, why } => write!(
                formatter,
                "the command's terminal {why}: what is typed could show in the output the agent \
                 reads, so nothing was typed; stop it with x (`torlek approval stop {request}`), \
                 or answer a visible prompt on purpose with Tab (`--shown`)"
            ),
            Refusal::NoSudoAsk { request, ask } => write!(
                formatter,
                "sudo's ask {ask} for request {request} no longer waits: nothing was sent"
            ),
            Refusal::SudoStoppedAsking { request, ask, why } => write!(
                formatter,
                "sudo's ask {ask} for request {request} was not answered: {why}"
            ),
            Refusal::SecretUnprotected { request, why } => write!(
                formatter,
                "the secret for request {request} could not be readied for redaction from its \
                 output ({why}): nothing was sent"
            ),
            Refusal::NotStoppable { why } => write!(formatter, "nothing was stopped: {why}"),
            Refusal::StopUnconfirmed { .. } => formatter.write_str(
                "the command was sent SIGTERM, then SIGKILL, but its processes were not observed \
                 gone",
            ),
            Refusal::NoStoredOutput { what } => write!(
                formatter,
                "the request is {what}; only a finished command run stores output"
            ),
            Refusal::StaleStep { current } => write!(
                formatter,
                "the guide is at step {}, not the one answered",
                current + 1
            ),
            Refusal::TooSoon { wait_seconds } => write!(
                formatter,
                "the request appeared or changed just now; an approval counts {wait_seconds} s \
                 from now"
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
    /// A transaction could not start or commit.
    Transaction(cockpit_turso::Error),
    /// `cockpit-processes` could not read or observe a run's execution.
    Processes(cockpit_processes::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused(refusal) => write!(formatter, "refused: {refusal}"),
            Error::InvalidStoredRow { table, reason } => write!(formatter, "{table}: {reason}"),
            Error::Authorization(error) => write!(formatter, "authorization: {error}"),
            Error::Database(error) => write!(formatter, "database: {error}"),
            Error::Transaction(error) => write!(formatter, "database: {error}"),
            Error::Processes(error) => write!(formatter, "processes: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Authorization(source) => Some(source),
            Error::Database(source) => Some(source),
            Error::Transaction(source) => Some(source),
            Error::Processes(source) => Some(source),
            Error::Refused(_) | Error::InvalidStoredRow { .. } => None,
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

impl From<cockpit_turso::Error> for Error {
    fn from(source: cockpit_turso::Error) -> Self {
        Error::Transaction(source)
    }
}

impl From<cockpit_processes::Error> for Error {
    fn from(source: cockpit_processes::Error) -> Self {
        Error::Processes(source)
    }
}

pub(crate) fn invalid(field: &'static str, reason: impl Into<String>) -> Refusal {
    Refusal::InvalidRequest {
        field,
        reason: reason.into(),
    }
}
