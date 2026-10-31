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
    /// A kind this version does not know. Read only: `decode_request` refuses it as
    /// `unknown-command`, and serializing it fails.
    #[serde(other, skip_serializing)]
    Unknown,
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
    match serde_json::from_str::<Request>(frame) {
        Ok(Request {
            id,
            command: Command::Unknown,
        }) => Err(RequestRejection {
            id: Some(id),
            error: ProtocolError {
                code: ErrorCode::UnknownCommand,
                message: format!(
                    "this daemon does not know the command `{}`",
                    command_kind(frame)
                ),
            },
        }),
        Ok(request) => Ok(request),
        Err(error) => Err(RequestRejection {
            id: readable_id(frame),
            error: ProtocolError {
                code: ErrorCode::InvalidRequest,
                message: format!("malformed request: {error}"),
            },
        }),
    }
}

/// The id of a frame that failed to parse as a request, when it has a valid one.
fn readable_id(frame: &str) -> Option<RequestId> {
    let value: Value = serde_json::from_str(frame).ok()?;
    RequestId::deserialize(value.get("id")?).ok()
}

/// Only called after the frame parsed with a string `kind`, which serde(other) requires.
fn command_kind(frame: &str) -> String {
    let value: Value = serde_json::from_str(frame).expect("the frame already parsed");
    value["command"]["kind"]
        .as_str()
        .expect("serde(other) matched a string kind")
        .to_owned()
}
