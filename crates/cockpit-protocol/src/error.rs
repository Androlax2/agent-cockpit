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
    /// The daemon has no capacity for this class of work right now; retrying later may work.
    Busy,
    /// The command was based on an entity revision that is no longer current.
    StaleRevision,
    /// The cursor's epoch or position is no longer retained; the client must take a snapshot.
    ResnapshotRequired,
    /// The daemon failed; the request may or may not have taken effect.
    Internal,
    /// A code this version does not know, kept as sent.
    #[serde(untagged)]
    Unknown(String),
}
