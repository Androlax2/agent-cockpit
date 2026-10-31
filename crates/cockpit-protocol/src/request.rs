use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::change_feed::{ChangeBatch, Cursor};
use crate::error::{ErrorCode, ProtocolError};

/// Correlates a [`Response`] with the [`Request`] it answers; chosen by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: RequestId,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Command {
    Ping,
    /// Start the change feed after `after`, or from a fresh snapshot when `None`.
    Subscribe {
        after: Option<Cursor>,
    },
}

impl Command {
    /// Every `kind` this version reads, so an unknown one is refused as `unknown-command`
    /// rather than as a malformed request.
    pub const KINDS: [&str; 2] = ["ping", "subscribe"];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub id: RequestId,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Ok(Reply),
    Error(ProtocolError),
}

/// What a command produced. A client only receives replies to commands it sent, so this set
/// is closed for each protocol version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Reply {
    Pong,
    Subscribed { cursor: Cursor },
}

/// Every frame the daemon sends after the handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ServerFrame {
    Response(Response),
    Changes(ChangeBatch),
    /// The answer to a frame that had no readable request id to answer.
    MalformedFrame(ProtocolError),
}

/// A request frame the daemon cannot carry out, with the id to answer when one was readable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRejection {
    pub id: Option<RequestId>,
    pub error: ProtocolError,
}

impl RequestRejection {
    /// The frame that tells the client about this rejection.
    pub fn into_frame(self) -> ServerFrame {
        match self.id {
            Some(id) => ServerFrame::Response(Response {
                id,
                outcome: Outcome::Error(self.error),
            }),
            None => ServerFrame::MalformedFrame(self.error),
        }
    }
}

/// Reads one request frame, telling an unknown command apart from a malformed one.
pub fn decode_request(frame: &str) -> Result<Request, RequestRejection> {
    let invalid = |id, message: String| RequestRejection {
        id,
        error: ProtocolError {
            code: ErrorCode::InvalidRequest,
            message,
        },
    };
    let value: Value = serde_json::from_str(frame)
        .map_err(|error| invalid(None, format!("the frame is not JSON: {error}")))?;
    let id = value
        .get("id")
        .and_then(|id| RequestId::deserialize(id).ok());
    let kind = value
        .get("command")
        .and_then(|command| command.get("kind"))
        .and_then(Value::as_str);
    if let Some(kind) = kind
        && !Command::KINDS.contains(&kind)
    {
        return Err(RequestRejection {
            id,
            error: ProtocolError {
                code: ErrorCode::UnknownCommand,
                message: format!("this daemon does not know the command `{kind}`"),
            },
        });
    }
    serde_json::from_value(value)
        .map_err(|error| invalid(id, format!("malformed request: {error}")))
}
