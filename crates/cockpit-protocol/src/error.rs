use serde::{Deserialize, Serialize};

/// Why the daemon did not carry out a request. Clients branch on `code`; `message` is for
/// people. A refusal the sender can act on also says what would lift it (report 17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unblock: Option<Unblock>,
}

impl ProtocolError {
    /// An error with no unblock: the daemon failed, or nothing the sender does changes it.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ProtocolError {
        ProtocolError {
            code,
            message: message.into(),
            unblock: None,
        }
    }
}

/// What would lift a refusal, and who can do it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unblock {
    pub action: String,
    pub by: UnblockedBy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnblockedBy {
    /// The human who owns the installation.
    Owner,
    /// The sender, by changing its request.
    Agent,
    /// Cockpit itself: a missing feature or a fault to report.
    Cockpit,
    /// A party this version does not know, kept as sent. Never build one with a known name.
    #[serde(untagged)]
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    /// The frame is not a well-formed request.
    InvalidRequest,
    /// The command kind is not one this daemon knows.
    UnknownCommand,
    /// The sender lacks the grant the command needs, or the command is the owner's alone.
    NotAuthorized,
    /// The agent socket could not tell which session the sender acts for.
    UnidentifiedPeer,
    /// The request is well formed, but a value in it is not acceptable.
    InvalidArgument,
    /// The command needs a capability the handshake did not grant.
    NotNegotiated,
    /// The daemon has no capacity for this class of work right now; retrying later may work.
    Busy,
    /// The command names an entity that does not exist.
    NotFound,
    /// The command conflicts with the entity's current state, such as another holder's
    /// occupancy or a name already taken.
    Conflict,
    /// A safety rule refused the command; the message names the rule.
    Refused,
    /// The command was based on an entity revision that is no longer current.
    StaleRevision,
    /// The cursor's epoch or position is no longer retained; the client must take a snapshot.
    ResnapshotRequired,
    /// The daemon failed; the request may or may not have taken effect.
    Internal,
    /// A code this version does not know, kept as sent. Never build one with a known name.
    #[serde(untagged)]
    Unknown(String),
}

impl ErrorCode {
    /// The code's name on the wire, such as `not-found`.
    pub fn as_str(&self) -> &str {
        match self {
            ErrorCode::InvalidRequest => "invalid-request",
            ErrorCode::UnknownCommand => "unknown-command",
            ErrorCode::NotAuthorized => "not-authorized",
            ErrorCode::UnidentifiedPeer => "unidentified-peer",
            ErrorCode::InvalidArgument => "invalid-argument",
            ErrorCode::NotNegotiated => "not-negotiated",
            ErrorCode::Busy => "busy",
            ErrorCode::NotFound => "not-found",
            ErrorCode::Conflict => "conflict",
            ErrorCode::Refused => "refused",
            ErrorCode::StaleRevision => "stale-revision",
            ErrorCode::ResnapshotRequired => "resnapshot-required",
            ErrorCode::Internal => "internal",
            ErrorCode::Unknown(code) => code,
        }
    }
}

/// Longest message built from a peer's input, so a huge frame cannot grow its own rejection.
const MAX_ECHOED_MESSAGE_BYTES: usize = 256;

/// Cuts `message` to [`MAX_ECHOED_MESSAGE_BYTES`] on a character boundary.
pub(crate) fn bounded(mut message: String) -> String {
    if message.len() > MAX_ECHOED_MESSAGE_BYTES {
        let mut end = MAX_ECHOED_MESSAGE_BYTES;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push_str("...");
    }
    message
}
