use serde::{Deserialize, Serialize};

/// Why the daemon did not carry out a request. Clients branch on `code`; `message` is for
/// people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    /// The frame is not a well-formed request.
    InvalidRequest,
    /// The command kind is not one this daemon knows.
    UnknownCommand,
    /// The sender lacks the grant the command needs.
    NotAuthorized,
    /// The command needs a capability the handshake did not grant.
    NotNegotiated,
    /// The daemon has no capacity for this class of work right now; retrying later may work.
    Busy,
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
