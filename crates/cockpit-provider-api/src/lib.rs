#![doc = include_str!("../README.md")]

mod capabilities;
mod commands;
mod events;
#[cfg(feature = "fake")]
pub mod fake;
mod sign_in;
mod terminal;

use std::ffi::OsString;
use std::fmt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

pub use capabilities::{Availability, Capabilities, Capability, Provenance};
pub use commands::{CommandResult, NativeCommand};
pub use events::{
    Activity, Confidence, EventKind, Observation, ObservedBy, ProviderEvent, TurnOutcome,
};
pub use sign_in::{SignIn, SignInStatus};
pub use terminal::{Action, Delivered, Key, TerminalInput};

/// Longest provider name; it is typed on the command line (`--agent`).
const MAX_NAME_BYTES: usize = 32;
/// Longest conversation id; providers use UUIDs or short thread ids.
const MAX_CONVERSATION_BYTES: usize = 256;

/// A provider's name, as the owner types it: lower-case ASCII letters, digits and `-`,
/// starting with a letter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProviderName(String);

impl ProviderName {
    pub fn new(name: impl Into<String>) -> Result<ProviderName, InvalidValue> {
        let name = name.into();
        let is_plain = name.len() <= MAX_NAME_BYTES
            && name.starts_with(|first: char| first.is_ascii_lowercase())
            && name.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            });
        if !is_plain {
            return Err(InvalidValue {
                kind: "provider name",
                value: name,
                rule: "1 to 32 lower-case ASCII letters, digits and `-`, starting with a letter",
            });
        }
        Ok(ProviderName(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The provider's own id of a conversation. Opaque; compared byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConversationId(String);

impl ConversationId {
    pub fn new(id: impl Into<String>) -> Result<ConversationId, InvalidValue> {
        let id = id.into();
        let is_plain = !id.is_empty()
            && id.len() <= MAX_CONVERSATION_BYTES
            && id.chars().all(|character| character.is_ascii_graphic());
        if !is_plain {
            return Err(InvalidValue {
                kind: "conversation id",
                value: id,
                rule: "1 to 256 printable ASCII characters, no spaces",
            });
        }
        Ok(ConversationId(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConversationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Which conversation a launch runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conversation {
    New,
    Resume(ConversationId),
}

#[derive(Debug, Clone)]
pub struct LaunchRequest<'request> {
    pub working_dir: &'request Path,
    /// The session's own directory, private to it and reachable by both its agent and the
    /// daemon: where a provider keeps what its delivery needs, such as a socket
    /// ([`Provider::deliver`]). It exists before the launch; its path is short enough for a
    /// unix socket inside it.
    pub session_dir: &'request Path,
    pub conversation: &'request Conversation,
    /// The owner's first message, submitted with a new conversation.
    pub first_message: Option<&'request str>,
    /// The reasoning effort, in the provider's own words; needs [`Capability::Effort`].
    pub effort: Option<&'request str>,
    /// Turn the provider's own remote control on; needs [`Capability::RemoteControl`].
    pub remote_control: bool,
}

impl LaunchRequest<'_> {
    /// The capabilities a provider must report to launch this request.
    pub fn needed_capabilities(&self) -> Vec<Capability> {
        let mut needed = Vec::new();
        if matches!(self.conversation, Conversation::Resume(_)) {
            needed.push(Capability::Resume);
        }
        if self.effort.is_some() {
            needed.push(Capability::Effort);
        }
        if self.remote_control {
            needed.push(Capability::RemoteControl);
        }
        needed
    }

    /// Refuses this request, naming the first capability it needs that `capabilities`
    /// does not report.
    pub fn require(&self, capabilities: &Capabilities) -> Result<(), ProviderError> {
        self.needed_capabilities()
            .into_iter()
            .try_for_each(|capability| capabilities.require(capability))
    }
}

/// The program that runs the conversation in its terminal. `environment` is all it gets
/// from the provider; the caller adds Cockpit's own variables.
#[derive(Debug, Clone)]
pub struct LaunchCommand {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
    /// The conversation this launch runs, when the provider knows it at launch; for a
    /// resumed conversation, the one requested.
    pub conversation: Option<ConversationId>,
}

/// The running session a delivery reaches ([`Provider::deliver`]).
#[derive(Clone, Copy)]
pub struct DeliveryTarget<'target> {
    /// The command retained by the host for this launch, when delivery needs its endpoint
    /// or credentials. Never reconstruct it from agent-writable files or agent reports.
    pub launch: Option<&'target LaunchCommand>,
    /// The session's own directory ([`LaunchRequest::session_dir`]). Its agent writes it, so
    /// whatever is found there, a socket or a link to one, is the agent's word.
    pub session_dir: &'target Path,
    /// How the caller opens a socket there.
    pub sockets: &'target dyn SessionSockets,
}

impl fmt::Debug for DeliveryTarget<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeliveryTarget")
            .field("session_dir", &self.session_dir)
            .finish_non_exhaustive()
    }
}

/// Opens sockets in a session's directory for the caller, who runs outside the sandbox (#442).
pub trait SessionSockets: Send + Sync {
    /// A connection to `socket`, only when the process listening on it is one of the
    /// session's own: the agent may have replaced the socket with a link to any socket the
    /// caller can reach, such as the daemon's own or an unsandboxed agent's.
    fn connect(&self, socket: &Path) -> Result<UnixStream, ProviderError>;
}

/// An agent provider's adapter.
pub trait Provider: Send + Sync + 'static {
    fn name(&self) -> &ProviderName;

    /// The harness version running, when the adapter discovered it.
    fn version(&self) -> Option<&str> {
        None
    }

    fn capabilities(&self) -> Capabilities;

    /// The harness's current local sign-in status, not host authorization or proof of
    /// model access. May block for at most ten seconds; run off async workers. Never
    /// includes raw command output, credentials or account identifiers.
    fn sign_in(&self) -> SignIn {
        SignIn::unknown(
            "this adapter cannot read sign-in status",
            format!("start {} in a terminal and check its sign-in", self.name()),
        )
    }

    /// The command for `request`. Must not block: it builds words, it starts nothing. It
    /// refuses a request that needs a capability it does not report
    /// ([`LaunchRequest::require`]).
    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError>;

    /// The terminal input that takes `action` while the agent is in `activity` (`None`:
    /// unknown). Refuses an action whose capability it does not report, and one the
    /// activity makes unsafe, such as typing into an open dialog.
    fn input(
        &self,
        action: &Action<'_>,
        activity: Option<Activity>,
    ) -> Result<Vec<TerminalInput>, ProviderError> {
        let _ = activity;
        Err(ProviderError::unsupported(action.capability()))
    }

    /// Performs `action` in the running `conversation` itself, through the harness's own
    /// interface reached in the session's directory, instead of typing into its terminal
    /// ([`Provider::input`]). Every socket it reaches there is opened through
    /// [`DeliveryTarget::sockets`], never by its path alone. Blocks for a few seconds at
    /// most, so callers run it off their async workers. Refuses an action whose capability
    /// it does not report; the default performs none, and callers use [`Provider::input`].
    fn deliver(
        &self,
        action: &Action<'_>,
        target: &DeliveryTarget<'_>,
        conversation: &ConversationId,
    ) -> Result<Delivered, ProviderError> {
        let _ = (target, conversation);
        Err(ProviderError::unsupported(action.capability()))
    }

    /// Lists native commands from this launch's endpoint. Requires `commands`; the
    /// capability carries the adapter's tested/discovered provenance, while each result
    /// keeps the harness's source label. Blocks for a few seconds; use an off-worker call.
    fn commands(&self, target: &DeliveryTarget<'_>) -> Result<Vec<NativeCommand>, ProviderError> {
        let _ = target;
        Err(ProviderError::unsupported(Capability::Commands))
    }

    /// Runs a native command in the named conversation. Requires `commands`. A reply is
    /// not proof the task completed. Blocks for a bounded wait, including the harness's
    /// reply; a failed wait does not prove the harness did not accept the command.
    fn invoke_command(
        &self,
        target: &DeliveryTarget<'_>,
        conversation: &ConversationId,
        command: &str,
        arguments: &str,
    ) -> Result<CommandResult, ProviderError> {
        let _ = (target, conversation, command, arguments);
        Err(ProviderError::unsupported(Capability::Commands))
    }

    /// The agent's activity read from the end of its terminal, for when no event says it
    /// ([`Capability::ScreenReading`]). Never proof that a task completed.
    fn read_screen(&self, screen: &str) -> Option<Observation> {
        let _ = screen;
        None
    }
}

/// The provider cannot launch the request; `reason` says why, for the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub reason: String,
}

impl ProviderError {
    /// An operation that needs `capability`, which the provider does not report.
    pub fn unsupported(capability: Capability) -> ProviderError {
        ProviderError {
            reason: format!(
                "it does not report the `{}` capability",
                capability.as_str()
            ),
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for ProviderError {}

/// A name or id that breaks its rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidValue {
    pub kind: &'static str,
    pub value: String,
    pub rule: &'static str,
}

impl fmt::Display for InvalidValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:?} is not a valid {}: {}",
            self.value, self.kind, self.rule
        )
    }
}

impl std::error::Error for InvalidValue {}
