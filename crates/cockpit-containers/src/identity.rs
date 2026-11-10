//! Whether the engine at a project's socket is that project's engine (#390, #856 T1): the
//! kernel names the user that listens there, the engine says it is rootless, and its ID is
//! the one the owner recorded. A socket path names no identity; anything could listen there.

use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::EngineUser;
use crate::info::{EngineInfo, parse_info};

/// How long the socket may take to answer before the engine counts as unreachable.
pub const SOCKET_DEADLINE: Duration = Duration::from_secs(10);

/// `/info` is a few kilobytes; a longer answer is no engine's.
const MAX_ANSWER_BYTES: u64 = 1024 * 1024;

/// What answered at an engine's socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The user the kernel says listens on the socket, as this process sees user ids.
    pub peer_uid: u32,
    pub engine: EngineInfo,
}

/// Asks the engine listening at `socket` who it is, within `deadline`: the listener's user
/// from the kernel, the rest from the engine's `GET /info`.
pub async fn ask_socket(socket: &Path, deadline: Duration) -> Result<Answer, String> {
    let shown = socket.display();
    match tokio::time::timeout(deadline, ask(socket)).await {
        Ok(answer) => answer.map_err(|reason| format!("{shown}: {reason}")),
        Err(_) => Err(format!(
            "{shown}: no answer within {}s",
            deadline.as_secs_f32()
        )),
    }
}

async fn ask(socket: &Path) -> Result<Answer, String> {
    let mut stream = UnixStream::connect(socket)
        .await
        .map_err(|error| format!("cannot connect: {error}"))?;
    let peer_uid = stream
        .peer_cred()
        .map_err(|error| format!("the kernel named no listener: {error}"))?
        .uid();
    // HTTP/1.0: the engine answers without chunks and closes the connection.
    stream
        .write_all(b"GET /info HTTP/1.0\r\nHost: docker\r\n\r\n")
        .await
        .map_err(|error| format!("cannot ask: {error}"))?;
    let mut answer = Vec::new();
    stream
        .take(MAX_ANSWER_BYTES + 1)
        .read_to_end(&mut answer)
        .await
        .map_err(|error| format!("cannot read the answer: {error}"))?;
    if answer.len() as u64 > MAX_ANSWER_BYTES {
        return Err(format!(
            "its answer is longer than {MAX_ANSWER_BYTES} bytes"
        ));
    }
    let body = info_body(&answer)?;
    let engine = parse_info(body).map_err(|reason| format!("GET /info: {reason}"))?;
    Ok(Answer { peer_uid, engine })
}

/// The body of a `200` answer.
fn info_body(answer: &[u8]) -> Result<&str, String> {
    let answer = std::str::from_utf8(answer).map_err(|_| "its answer is not UTF-8".to_owned())?;
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .ok_or_else(|| "its answer is not HTTP".to_owned())?;
    let status_line = head.lines().next().unwrap_or_default();
    let status = status_line.split_whitespace().nth(1);
    if !status_line.starts_with("HTTP/") || status != Some("200") {
        let shown: String = status_line.chars().take(80).collect();
        return Err(format!("GET /info answered {shown:?}"));
    }
    Ok(body)
}

/// The engine the owner recorded for a project (`torlek containers record`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedEngine {
    pub project: String,
    pub user: String,
    pub engine_id: String,
    /// Seconds since 1970.
    pub recorded_at: i64,
    /// `cli` or `tui`; `None` for a client that did not name itself.
    pub recorded_through: Option<String>,
}

/// What a project's engine must be.
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    pub user: &'a EngineUser,
    /// The engine user's id from the accounts file; `None` when there is no such user.
    pub user_uid: Option<u32>,
    /// The daemon's own user: an engine running as it is the owner's engine.
    pub owner_uid: u32,
}

/// Where a project's engine stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineState {
    /// Nothing is recorded and no engine answers as the project's: the setup step is next.
    NotSetUp {
        reason: String,
    },
    /// The project's engine answers, rootless, as its own user; the owner has not recorded it.
    Unrecorded(EngineInfo),
    /// The engine the owner recorded answers as the project's own, rootless.
    Verified(EngineInfo),
    Refused(Refusal),
}

/// Why the engine at a project's socket is not used, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The engine user is gone.
    NoUser { user: String },
    /// The recorded engine does not answer.
    Unreachable { reason: String },
    /// Root listens on the socket: a rootful engine, whose user is root on the host.
    Root,
    /// The daemon's own user listens on the socket: the owner's engine.
    Owner { uid: u32 },
    /// Some other user listens on the socket.
    OtherUser {
        uid: u32,
        user: String,
        user_uid: u32,
    },
    /// The engine does not say it is rootless.
    Rootful { engine_id: String },
    /// The owner recorded the engine under another user.
    RecordedForAnotherUser { recorded: String, user: String },
    /// The engine is not the one the owner recorded.
    NotRecorded { found: String, recorded: String },
}

/// What makes a refused engine usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    /// Run the setup step again: it makes the user and restarts its unit.
    Setup,
    /// Start the engine's unit.
    Start,
    /// The owner records the engine that answers now, if it is the one they set up.
    Record,
}

impl Refusal {
    pub fn remedy(&self) -> Remedy {
        match self {
            Refusal::Unreachable { .. } => Remedy::Start,
            Refusal::RecordedForAnotherUser { .. } | Refusal::NotRecorded { .. } => Remedy::Record,
            Refusal::NoUser { .. }
            | Refusal::Root
            | Refusal::Owner { .. }
            | Refusal::OtherUser { .. }
            | Refusal::Rootful { .. } => Remedy::Setup,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NoUser { user } => write!(formatter, "the engine user {user} does not exist"),
            Refusal::Unreachable { reason } => {
                write!(formatter, "the recorded engine does not answer: {reason}")
            }
            Refusal::Root => formatter.write_str(
                "root listens on the engine's socket: a rootful engine is root on this host",
            ),
            Refusal::Owner { uid } => write!(
                formatter,
                "your own user (uid {uid}) listens on the engine's socket: your engine sees \
                 everything you can"
            ),
            Refusal::OtherUser {
                uid,
                user,
                user_uid,
            } => write!(
                formatter,
                "uid {uid} listens on the engine's socket, not {user} (uid {user_uid})"
            ),
            Refusal::Rootful { engine_id } => {
                write!(formatter, "engine {engine_id} does not say it is rootless")
            }
            Refusal::RecordedForAnotherUser { recorded, user } => write!(
                formatter,
                "the recorded engine ran as {recorded}; this project's engine user is {user}"
            ),
            Refusal::NotRecorded { found, recorded } => write!(
                formatter,
                "engine {found} answers, not the recorded {recorded}"
            ),
        }
    }
}

/// Where a project's engine stands, from what answered at its socket.
pub fn verify(
    answer: Result<Answer, String>,
    expected: Expected<'_>,
    recorded: Option<&RecordedEngine>,
) -> EngineState {
    let user = expected.user.name();
    let Some(user_uid) = expected.user_uid else {
        return match recorded {
            Some(_) => EngineState::Refused(Refusal::NoUser {
                user: user.to_owned(),
            }),
            None => EngineState::NotSetUp {
                reason: format!("there is no user {user} on this host"),
            },
        };
    };
    let answer = match (answer, recorded) {
        (Ok(answer), _) => answer,
        (Err(reason), Some(_)) => return EngineState::Refused(Refusal::Unreachable { reason }),
        (Err(reason), None) => {
            return EngineState::NotSetUp {
                reason: format!("no engine answers: {reason}"),
            };
        }
    };
    if let Err(refusal) = check_identity(&answer, expected, user_uid) {
        return EngineState::Refused(refusal);
    }
    let Some(recorded) = recorded else {
        return EngineState::Unrecorded(answer.engine);
    };
    if recorded.user != user {
        return EngineState::Refused(Refusal::RecordedForAnotherUser {
            recorded: recorded.user.clone(),
            user: user.to_owned(),
        });
    }
    if recorded.engine_id != answer.engine.id {
        return EngineState::Refused(Refusal::NotRecorded {
            found: answer.engine.id,
            recorded: recorded.engine_id.clone(),
        });
    }
    EngineState::Verified(answer.engine)
}

/// Who listens, and that it is rootless.
fn check_identity(answer: &Answer, expected: Expected<'_>, user_uid: u32) -> Result<(), Refusal> {
    match answer.peer_uid {
        0 => return Err(Refusal::Root),
        uid if uid == expected.owner_uid => return Err(Refusal::Owner { uid }),
        uid if uid != user_uid => {
            return Err(Refusal::OtherUser {
                uid,
                user: expected.user.name().to_owned(),
                user_uid,
            });
        }
        _ => {}
    }
    if !answer.engine.rootless {
        return Err(Refusal::Rootful {
            engine_id: answer.engine.id.clone(),
        });
    }
    Ok(())
}
