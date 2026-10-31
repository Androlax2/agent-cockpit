#![doc = include_str!("../README.md")]

mod capabilities;
mod events;
#[cfg(feature = "fake")]
pub mod fake;
mod terminal;

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

pub use capabilities::{Availability, Capabilities, Capability, Provenance};
pub use events::{
    Activity, Confidence, EventKind, Observation, ObservedBy, ProviderEvent, TurnOutcome,
};
pub use terminal::{Action, Key, TerminalInput};

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

/// An agent provider's adapter.
pub trait Provider: Send + Sync + 'static {
    fn name(&self) -> &ProviderName;

    /// The harness version running, when the adapter discovered it.
    fn version(&self) -> Option<&str> {
        None
    }

    fn capabilities(&self) -> Capabilities;

    /// The command for `request`. Must not block: it builds words, it starts nothing. It
    /// refuses a request that needs a capability it does not report
    /// ([`LaunchRequest::needed_capabilities`]).
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
