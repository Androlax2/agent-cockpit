use std::fmt;
use std::path::PathBuf;

use cockpit_processes::ExecutionId;

use crate::{SessionId, SessionState, SleepRefusal};

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    InvalidSessionId {
        id: String,
    },
    InvalidLink {
        link: String,
    },
    /// A name or id the provider contract refused.
    InvalidValue(cockpit_provider_api::InvalidValue),
    UnknownSession {
        session: SessionId,
    },
    UnknownProvider {
        name: String,
        available: Vec<String>,
    },
    /// No provider was named and there is not exactly one to choose.
    ProviderNeeded {
        available: Vec<String>,
    },
    /// The directory is in no registered project's checkout.
    NotInProject {
        dir: PathBuf,
    },
    /// The operation needs another state; `operation` names it for the owner.
    WrongState {
        session: SessionId,
        state: SessionState,
        operation: &'static str,
    },
    /// The conversation cannot be launched again; `reason` says why.
    CannotResume {
        session: SessionId,
        reason: String,
    },
    SleepRefused {
        session: SessionId,
        refusal: SleepRefusal,
    },
    /// A stop came while a launch holds the session; it is refused rather than wait (#230).
    LaunchInProgress {
        session: SessionId,
        operation: &'static str,
    },
    /// The agent's execution was not observed gone after `SIGKILL`; the session keeps it.
    StopUnconfirmed {
        session: SessionId,
        execution: ExecutionId,
    },
    Provider(cockpit_provider_api::ProviderError),
    Projects(cockpit_projects::Error),
    Processes(cockpit_processes::Error),
    Tmux(cockpit_tmux::Error),
    /// Cockpit's tmux server could not be opened when the daemon started; `reason` says why.
    TmuxUnavailable {
        reason: String,
    },
    AgentChannel(cockpit_agent_channel::Error),
    Authorization(cockpit_authorization::Error),
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    /// A stored row breaks this crate's schema rules.
    InvalidStoredRow {
        reason: String,
    },
    Database(cockpit_turso::Error),
    Sql(cockpit_turso::sql::Error),
    /// A blocking tmux or filesystem step panicked.
    Blocking(String),
    /// A blocking step did not return within the step timeout; it may still finish (#272).
    StepTimedOut {
        step: &'static str,
        after: std::time::Duration,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidSessionId { id } => write!(
                formatter,
                "{id:?} is not a session id: `s` and 12 lower-case hex digits"
            ),
            Error::InvalidLink { link } => write!(
                formatter,
                "{link:?} is not a design link: an http or https URL without spaces"
            ),
            Error::InvalidValue(invalid) => invalid.fmt(formatter),
            Error::UnknownSession { session } => write!(formatter, "no session has id {session}"),
            Error::UnknownProvider { name, available } => write!(
                formatter,
                "no agent is named {name:?}; available: {}",
                list(available)
            ),
            Error::ProviderNeeded { available } => write!(
                formatter,
                "name the agent with --agent; available: {}",
                list(available)
            ),
            Error::NotInProject { dir } => write!(
                formatter,
                "{} is in no registered project; register it with `cockpit project add`",
                dir.display()
            ),
            Error::WrongState {
                session,
                state,
                operation,
            } => write!(
                formatter,
                "session {session} is {state}; cannot {operation}"
            ),
            Error::CannotResume { session, reason } => {
                write!(formatter, "session {session} cannot be resumed: {reason}")
            }
            Error::SleepRefused { session, refusal } => {
                write!(formatter, "session {session} may not sleep: {refusal}")
            }
            Error::LaunchInProgress { session, operation } => write!(
                formatter,
                "session {session} is launching or restarting its agent; cannot {operation} \
                 until that finishes, retry then"
            ),
            Error::StopUnconfirmed { session, execution } => write!(
                formatter,
                "session {session}'s agent did not stop: {execution} still has processes \
                 after SIGKILL; the session keeps it"
            ),
            Error::Provider(error) => write!(formatter, "the agent's provider: {error}"),
            Error::Projects(error) => error.fmt(formatter),
            Error::Processes(error) => write!(formatter, "processes: {error}"),
            Error::Tmux(error) => write!(formatter, "tmux: {error}"),
            Error::TmuxUnavailable { reason } => write!(
                formatter,
                "sessions need tmux, which was unavailable when the daemon started: {reason}; \
                 install tmux, then restart the daemon"
            ),
            Error::AgentChannel(error) => write!(formatter, "agent channel: {error}"),
            Error::Authorization(error) => write!(formatter, "authorization: {error}"),
            Error::Io {
                action,
                path,
                source,
            } => write!(formatter, "could not {action} {}: {source}", path.display()),
            Error::InvalidStoredRow { reason } => write!(formatter, "stored session: {reason}"),
            Error::Database(error) => write!(formatter, "database: {error}"),
            Error::Sql(error) => write!(formatter, "database: {error}"),
            Error::Blocking(reason) => write!(formatter, "a blocking step failed: {reason}"),
            Error::StepTimedOut { step, after } => write!(
                formatter,
                "the step to {step} did not finish within {after:?}; it may still finish"
            ),
        }
    }
}

impl Error {
    /// Whether the state database failed, directly or under the capability that reported
    /// it: the one failure that stops the daemon's start in reboot restore (#251).
    pub(crate) fn is_database(&self) -> bool {
        matches!(
            self,
            Error::Database(_)
                | Error::Sql(_)
                | Error::Processes(cockpit_processes::Error::Database(_))
                | Error::Authorization(cockpit_authorization::Error::Database(_))
                | Error::AgentChannel(cockpit_agent_channel::Error::Database(_))
                | Error::Projects(
                    cockpit_projects::Error::Database(_) | cockpit_projects::Error::Sql(_)
                )
        )
    }
}

fn list(names: &[String]) -> String {
    if names.is_empty() {
        return "none".to_owned();
    }
    names.join(", ")
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Provider(source) => Some(source),
            Error::Projects(source) => Some(source),
            Error::Processes(source) => Some(source),
            Error::Tmux(source) => Some(source),
            Error::AgentChannel(source) => Some(source),
            Error::Authorization(source) => Some(source),
            Error::Io { source, .. } => Some(source),
            Error::Database(source) => Some(source),
            Error::Sql(source) => Some(source),
            _ => None,
        }
    }
}

macro_rules! from_source {
    ($($source:ty => $variant:ident),* $(,)?) => {
        $(impl From<$source> for Error {
            fn from(source: $source) -> Error {
                Error::$variant(source)
            }
        })*
    };
}

from_source! {
    cockpit_provider_api::InvalidValue => InvalidValue,
    cockpit_provider_api::ProviderError => Provider,
    cockpit_projects::Error => Projects,
    cockpit_processes::Error => Processes,
    cockpit_tmux::Error => Tmux,
    cockpit_agent_channel::Error => AgentChannel,
    cockpit_authorization::Error => Authorization,
    cockpit_turso::Error => Database,
    cockpit_turso::sql::Error => Sql,
}
