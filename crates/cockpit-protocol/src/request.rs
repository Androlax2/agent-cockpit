use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::change_feed::{ChangeBatch, Cursor, FeedPosition};
use crate::error::{ErrorCode, ProtocolError, bounded};

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
    /// Start the change feed at `from`.
    Subscribe {
        from: FeedPosition,
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
    let value: Value = serde_json::from_str(frame).map_err(|error| {
        rejection(
            None,
            ErrorCode::InvalidRequest,
            format!("the frame is not JSON: {error}"),
        )
    })?;
    let id = value
        .get("id")
        .and_then(|id| RequestId::deserialize(id).ok());
    match Request::deserialize(&value) {
        Ok(Request {
            command: Command::Unknown,
            ..
        }) => match value["command"]["kind"].as_str() {
            Some(kind) => Err(rejection(
                id,
                ErrorCode::UnknownCommand,
                format!("this daemon does not know the command `{kind}`"),
            )),
            None => Err(rejection(
                id,
                ErrorCode::InvalidRequest,
                "the command kind is not a string".to_owned(),
            )),
        },
        Ok(request) => Ok(request),
        Err(error) => Err(rejection(
            id,
            ErrorCode::InvalidRequest,
            format!("malformed request: {error}"),
        )),
    }
}

fn rejection(id: Option<RequestId>, code: ErrorCode, message: String) -> RequestRejection {
    RequestRejection {
        id,
        error: ProtocolError {
            code,
            message: bounded(message),
        },
    }
}
