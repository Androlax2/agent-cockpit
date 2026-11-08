#![doc = include_str!("../README.md")]

mod capabilities;
mod commands;
mod events;
#[cfg(feature = "fake")]
pub mod fake;
mod needs;
mod probe;
mod quota;
mod sign_in;
mod terminal;
mod tested_versions;
pub mod toolchains;

use std::ffi::OsString;
use std::fmt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

pub use capabilities::{Availability, Capabilities, Capability, Provenance};
pub use commands::{CommandResult, NativeCommand};
pub use events::{
    Activity, CompactionMoment, CompactionPhase, CompactionTrigger, Confidence, ContextUsage,
    EventKind, Observation, ObservedBy, ProviderEvent, TurnOutcome,
};
pub use needs::{
    CredentialBinding, CredentialHeader, Credentials, HomePath, Host, Needs, is_credential_header,
};
pub use probe::{
    Missing, Probe, ProbeError, command_output, documents_flag, names_missing_from_file,
};
pub use quota::{LimitKey, MAX_WINDOWS, QuotaReading, QuotaWindow};
pub use sign_in::{SignIn, SignInStatus};
pub use terminal::{Action, Delivered, Key, TerminalInput};
pub use tested_versions::parse_tested_versions;

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
    /// The model, in the provider's own words (an alias or a full id); needs
    /// [`Capability::Model`].
    pub model: Option<&'request str>,
    /// Turn the provider's own remote control on; needs [`Capability::RemoteControl`].
    pub remote_control: bool,
    /// The harness's own compaction threshold, in the harness's own words (a context window
    /// for Claude Code, a percent for Codex, a buffer for OpenCode); needs
    /// [`Capability::CompactionThreshold`]. `None` leaves the harness's own setting alone.
    pub compaction_threshold: Option<&'request str>,
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
        if self.model.is_some() {
            needed.push(Capability::Model);
        }
        if self.remote_control {
            needed.push(Capability::RemoteControl);
        }
        if self.compaction_threshold.is_some() {
            needed.push(Capability::CompactionThreshold);
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

    /// What the runtime probe found in the version running, when it ran: only for a version
    /// not on the adapter's tested list (#732).
    fn probe(&self) -> Option<&Probe> {
        None
    }

    /// The hosts and home folders its harness needs from the sandbox (#830). The default is
    /// the floor: no host and no folder beyond the checkout and the session's own. Refuses a
    /// malformed declaration instead of narrowing it.
    fn needs(&self) -> Result<Needs, InvalidValue> {
        Ok(Needs::none())
    }

    /// Where its harness keeps and takes credentials (#858), checked against
    /// [`Provider::needs`]. The default declares none.
    fn credentials(&self) -> Result<Credentials, InvalidValue> {
        Ok(Credentials::none())
    }

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

    /// Refuses a model or effort the installed harness does not have, asking the harness
    /// itself (its help, a list command or its server) instead of a list of Torlek's own, so
    /// a level or model a harness update adds works without a Torlek release (#743). Said
    /// with the harness's own words and version. A harness that lists nothing passes the
    /// choice through for itself to refuse. Run after the shape checks of [`Provider::launch`],
    /// before a session is recorded; may block for a few seconds, so callers run it off their
    /// async workers. The default accepts.
    fn check_choices(
        &self,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Result<(), ProviderError> {
        let _ = (model, effort);
        Ok(())
    }

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
    /// it does not report. Only [`Delivered::Unsupported`] permits fallback to
    /// [`Provider::input`]; errors never do, since the action may already have reached the
    /// harness. The default checks the capability and reports unsupported delivery.
    fn deliver(
        &self,
        action: &Action<'_>,
        target: &DeliveryTarget<'_>,
        conversation: &ConversationId,
    ) -> Result<Delivered, ProviderError> {
        let _ = (target, conversation);
        self.capabilities().require(action.capability())?;
        Ok(Delivered::Unsupported)
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

    /// Whether its bridge reports as soon as the harness starts, before any prompt (#702):
    /// a launch that stays silent then waits on a screen of the harness's own (folder trust,
    /// sign-in, update). `false` for a harness whose first event needs a first prompt.
    fn reports_at_launch(&self) -> bool {
        false
    }

    /// Records the harness's own trust of `request.workspace` before a launch there, so the
    /// harness does not stop on its folder-trust dialog before its agent runs (#702). Torlek
    /// calls it only for a parallel clone it checked may inherit its project's trust; the
    /// adapter refuses when the clone brings harness settings the trusted checkout lacks,
    /// revoking a trust it kept from an earlier launch, or when the harness's record cannot
    /// be written safely, writing nothing. A refusal does not
    /// stop the launch: the dialog then shows, and the session waits on the owner. Reads and
    /// writes files: run it off the async workers. The default keeps no trust.
    fn trust_workspace(&self, request: &TrustRequest<'_>) -> Result<WorkspaceTrust, ProviderError> {
        let _ = request;
        Ok(WorkspaceTrust::NotKept)
    }

    /// Answers, in a sandboxed session's own home, the screens the harness shows on a first
    /// start there and nobody is at the terminal to answer (#1178): a session's home starts
    /// empty, so every sandboxed launch would stop on them. Never a decision the owner keeps
    /// for a project, such as folder trust ([`Provider::trust_workspace`]). Torlek calls it
    /// before each sandboxed launch; a refusal does not stop the launch: the screen then
    /// shows, and the session waits on the owner. Reads and writes files: run it off the
    /// async workers. The default writes nothing.
    fn prepare_session_home(&self, request: &SessionHomeRequest<'_>) -> Result<(), ProviderError> {
        let _ = request;
        Ok(())
    }

    /// The plan windows of the account the harness is signed in to under `home`, the
    /// owner's home folder, never a session's ([`Capability::Quota`], #58). Run by the host
    /// daemon only: the credential it reads to ask stays in the adapter, never in an error,
    /// a log or a session (#858). May block for at most fifteen seconds; run off async
    /// workers. The default refuses.
    fn quota(&self, home: &Path) -> Result<QuotaReading, ProviderError> {
        let _ = home;
        Err(ProviderError::unsupported(Capability::Quota))
    }

    /// The agent's activity read from the end of its terminal, for when no event says it
    /// ([`Capability::ScreenReading`]). Never proof that a task completed.
    fn read_screen(&self, screen: &str) -> Option<Observation> {
        let _ = screen;
        None
    }
}

/// A workspace whose harness trust [`Provider::trust_workspace`] records.
#[derive(Debug, Clone, Copy)]
pub struct TrustRequest<'a> {
    /// The checkout's root: a parallel clone, or under the sandbox a trusted main checkout.
    pub workspace: &'a Path,
    /// Where the agent starts, inside the checkout: the harness reads settings there too.
    pub working_dir: &'a Path,
    /// The project's main checkout, whose trust the workspace inherits; the workspace itself
    /// when it is that checkout.
    pub trusted_checkout: &'a Path,
    /// The environment the agent launches with, which locates the harness's own files.
    pub environment: &'a [(OsString, OsString)],
    /// The folder that backs the agent's home when it runs sandboxed, its session's own
    /// (#858, #968): the harness's files under `HOME` are there, and the adapter may create
    /// them. `None` when the agent's home is the owner's.
    pub session_home: Option<&'a Path>,
}

/// A sandboxed session's home that [`Provider::prepare_session_home`] prepares.
#[derive(Debug, Clone, Copy)]
pub struct SessionHomeRequest<'a> {
    /// The environment the agent launches with, which locates the harness's own files.
    pub environment: &'a [(OsString, OsString)],
    /// The folder that backs the agent's home, its session's own (#858, #968): the
    /// harness's files under `HOME` are there, and the adapter may create them.
    pub session_home: &'a Path,
    /// What the agent holds in its provider's credential variable when the session proxy
    /// adds the owner's credential (#968); `None` when no credential is bound.
    pub credential_placeholder: Option<&'a str>,
}

/// What [`Provider::trust_workspace`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceTrust {
    /// The harness keeps no folder trust this adapter records.
    NotKept,
    /// The harness already trusted the workspace.
    AlreadyTrusted,
    /// The harness's trust of the workspace was recorded.
    Recorded,
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

    /// An operation that needs `capability`, which the provider withholds for `reason`.
    pub fn withheld(capability: Capability, reason: &str) -> ProviderError {
        ProviderError {
            reason: format!(
                "it does not report the `{}` capability: {reason}",
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
