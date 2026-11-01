//! Actions performed in a running Codex conversation through its session's private
//! app-server (#407): a message starts a turn (`turn/start`) or joins the one under way
//! (`turn/steer`), a cancel interrupts it (`turn/interrupt`). The conversation is named by
//! its thread id: the TUI also loads helper threads of its own (measured on 0.161.0).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cockpit_provider_api::{Action, ConversationId, Delivered, ProviderError};
use serde_json::{Value, json};

use crate::app_server::{AppServer, RpcError};

/// The app-server's socket in the session's directory.
pub const SOCKET_FILE: &str = "codex.sock";

/// How long one call to the app-server may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a refused turn listing is retried, and the pause between tries.
const LIST_RETRY: Duration = Duration::from_secs(2);
const LIST_RETRY_PAUSE: Duration = Duration::from_millis(100);

/// The app-server socket of the session whose directory is `session_dir`.
pub fn socket_path(session_dir: &Path) -> PathBuf {
    session_dir.join(SOCKET_FILE)
}

/// Performs `action` in `conversation` through the app-server listening on `socket`.
pub fn deliver(
    socket: &Path,
    action: &Action<'_>,
    conversation: &ConversationId,
) -> Result<Delivered, ProviderError> {
    let thread = conversation.as_str();
    let failed = |error: RpcError| ProviderError {
        reason: error.reason,
    };
    match action {
        Action::Submit(message) => {
            let mut server = AppServer::connect(socket, "cockpit", CALL_TIMEOUT).map_err(failed)?;
            submit(&mut server, thread, message).map_err(failed)
        }
        Action::Cancel => {
            let mut server = AppServer::connect(socket, "cockpit", CALL_TIMEOUT).map_err(failed)?;
            let Some(turn) = turn_under_way(&mut server, thread).map_err(failed)? else {
                return Err(ProviderError {
                    reason: format!("no turn is under way in conversation {thread}"),
                });
            };
            server
                .call(
                    "turn/interrupt",
                    json!({"threadId": thread, "turnId": turn}),
                )
                .map_err(failed)?;
            Ok(Delivered::Interrupted)
        }
        Action::Compact => Err(ProviderError::unsupported(action.capability())),
    }
}

/// Joins the turn under way, or starts one when none is or it ended meanwhile.
fn submit(server: &mut AppServer, thread: &str, message: &str) -> Result<Delivered, RpcError> {
    let input = json!([{"type": "text", "text": message}]);
    if let Some(turn) = turn_under_way(server, thread)? {
        let steered = server.call(
            "turn/steer",
            json!({"threadId": thread, "expectedTurnId": turn, "input": input}),
        );
        if steered.is_ok() {
            return Ok(Delivered::JoinedTurn);
        }
        // The turn ended between the two calls; a turn under way again is an error.
        if turn_under_way(server, thread)?.is_some() {
            return steered.map(|_| Delivered::JoinedTurn);
        }
    }
    server.call("turn/start", json!({"threadId": thread, "input": input}))?;
    Ok(Delivered::StartedTurn)
}

/// The id of the turn under way in the thread, if one is. The thread's status says whether
/// one is: its turns cannot be listed before its first message (measured on 0.161.0).
fn turn_under_way(server: &mut AppServer, thread: &str) -> Result<Option<String>, RpcError> {
    let read = server.call("thread/read", json!({"threadId": thread}))?;
    if read.pointer("/thread/status/type").and_then(Value::as_str) != Some("active") {
        return Ok(None);
    }
    let last = last_turn(server, thread)?;
    Ok(last
        .filter(|turn| turn.get("status").and_then(Value::as_str) == Some("inProgress"))
        .and_then(|turn| turn.get("id").and_then(Value::as_str).map(str::to_owned)))
}

/// The thread's most recent turn, if it has one. For a moment after a thread's first message,
/// Codex refuses to list its turns (measured on 0.161.0: about 100 ms), so a refusal is
/// retried for up to [`LIST_RETRY`].
pub(crate) fn last_turn(server: &mut AppServer, thread: &str) -> Result<Option<Value>, RpcError> {
    let ends_at = Instant::now() + LIST_RETRY;
    let page = loop {
        let listed = server.call(
            "thread/turns/list",
            json!({"threadId": thread, "limit": 1, "sortDirection": "desc"}),
        );
        match listed {
            Ok(page) => break page,
            Err(_) if Instant::now() < ends_at => std::thread::sleep(LIST_RETRY_PAUSE),
            Err(error) => return Err(error),
        }
    };
    Ok(page
        .get("data")
        .and_then(Value::as_array)
        .and_then(|turns| turns.first())
        .cloned())
}
