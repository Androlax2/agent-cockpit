use std::fmt;
use std::path::PathBuf;

use cockpit_processes::ExecutionId;

use crate::{SessionId, SessionState, SleepRefusal};

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    InvalidMessage {
        reason: &'static str,
    },
    UnknownConversation {
        session: SessionId,
    },
    /// A provider call failed or timed out; it may already have sent the action.
    DeliveryUnconfirmed {
        session: SessionId,
        reason: String,
    },
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
    /// The sandbox refused the launch: a rule this host cannot enforce, or no engine (#61).
    Sandbox(cockpit_sandbox::Refusal),
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
    /// Every blocking step permit is held, by steps still running or timed out and not yet
    /// returned (#325); retrying later may work.
    StepsBusy {
        step: &'static str,
        limit: usize,
    },
    /// A launch would open the session's terminal while an earlier step on it, one that
    /// timed out, has not returned and may still close or open it (#324).
    TerminalBusy {
        session: SessionId,
    },
    /// Another operation on the session is under way; a message is not typed meanwhile.
    SessionBusy {
        session: SessionId,
    },
    /// Typing into the agent's terminal failed or was not seen to finish: the keys may have
    /// reached it, in part or whole.
    TypingUnconfirmed {
        session: SessionId,
        reason: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidMessage { reason } => write!(formatter, "invalid message: {reason}"),
            Error::UnknownConversation { session } => {
                write!(formatter, "session {session}'s conversation is unknown")
            }
            Error::DeliveryUnconfirmed { session, reason } => write!(
                formatter,
                "delivery to session {session} was not confirmed; the action may have reached it: {reason}"
            ),
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
            Error::Sandbox(refusal) => refusal.fmt(formatter),
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
            Error::StepsBusy { step, limit } => write!(
                formatter,
                "cannot {step}: all {limit} blocking steps are still running, some may be \
                 hung in tmux or a provider; retry later"
            ),
            Error::TerminalBusy { session } => write!(
                formatter,
                "session {session}'s terminal cannot be opened: an earlier step on it timed \
                 out and has not returned yet; retry once tmux answers"
            ),
            Error::SessionBusy { session } => write!(
                formatter,
                "session {session} is busy with another operation; retry once it finishes"
            ),
            Error::TypingUnconfirmed { session, reason } => write!(
                formatter,
                "typing into session {session}'s terminal was not confirmed, so the agent may \
                 have received the message: {reason}"
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
