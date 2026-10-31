use std::fmt;
use std::io;
use std::path::PathBuf;

use cockpit_protocol::{Capability, Cursor, FeedPosition, ProtocolError, Refusal, Reply};

#[derive(Debug)]
pub enum ClientError {
    /// The daemon's socket could not be reached.
    Connect {
        socket_path: PathBuf,
        source: io::Error,
    },
    /// The daemon refused the handshake.
    Refused(Refusal),
    /// The daemon did not grant a capability the operation needs.
    NotGranted(Capability),
    /// The caller's deadline passed. The client stopped waiting; the operation may or may not
    /// have taken effect.
    DeadlineExceeded,
    /// The connection is closed. Requests that were in flight may or may not have taken effect.
    Closed(CloseReason),
    /// The daemon carried out nothing, or did not say; the code tells which.
    Daemon(ProtocolError),
    /// The daemon answered with a reply that does not belong to the command.
    UnexpectedReply(Reply),
    /// A change batch did not follow the subscription's applied cursor, so a batch was
    /// missed. The subscription dropped its connection and resubscribes on the next call.
    FeedGap {
        applied: Option<Cursor>,
        follows: FeedPosition,
    },
    /// The outgoing frame cannot be serialized, such as a `Command::Unknown`.
    Encode(serde_json::Error),
    /// The outgoing frame is larger than [`crate::MAX_FRAME_BYTES`].
    FrameTooLarge,
}

/// Why a connection stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// The daemon closed the connection between two frames.
    Disconnected,
    /// The daemon closed the connection in the middle of a frame; the partial frame is dropped.
    Truncated,
    /// The daemon sent a frame larger than [`crate::MAX_FRAME_BYTES`].
    FrameTooLarge,
    /// The daemon sent a frame this client cannot read.
    InvalidFrame(String),
    /// The daemon could not read one of this client's frames.
    RejectedFrame(ProtocolError),
    /// Reading or writing the socket failed.
    Io(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect {
                socket_path,
                source,
            } => write!(
                formatter,
                "cannot connect to the daemon at {}: {source}",
                socket_path.display()
            ),
            Self::Refused(refusal) => write!(
                formatter,
                "the daemon refused the handshake ({:?}): {}",
                refusal.reason, refusal.message
            ),
            Self::NotGranted(capability) => {
                write!(formatter, "the daemon did not grant {capability:?}")
            }
            Self::DeadlineExceeded => formatter
                .write_str("the deadline passed; the operation may or may not have taken effect"),
            Self::Closed(reason) => write!(formatter, "the connection is closed: {reason}"),
            Self::Daemon(error) => write!(formatter, "{:?}: {}", error.code, error.message),
            Self::UnexpectedReply(reply) => {
                write!(formatter, "the daemon sent an unexpected reply: {reply:?}")
            }
            Self::FeedGap { applied, follows } => write!(
                formatter,
                "a change batch follows {follows:?}, not the applied cursor {applied:?}"
            ),
            Self::Encode(error) => write!(formatter, "cannot encode the request: {error}"),
            Self::FrameTooLarge => write!(
                formatter,
                "the request is larger than {} bytes",
                crate::MAX_FRAME_BYTES
            ),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect { source, .. } => Some(source),
            Self::Encode(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Display for CloseReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected => formatter.write_str("the daemon disconnected"),
            Self::Truncated => {
                formatter.write_str("the daemon disconnected in the middle of a frame")
            }
            Self::FrameTooLarge => write!(
                formatter,
                "the daemon sent a frame larger than {} bytes",
                crate::MAX_FRAME_BYTES
            ),
            Self::InvalidFrame(detail) => write!(formatter, "unreadable frame: {detail}"),
            Self::RejectedFrame(error) => write!(
                formatter,
                "the daemon could not read a frame ({:?}): {}",
                error.code, error.message
            ),
            Self::Io(detail) => write!(formatter, "socket error: {detail}"),
        }
    }
}
