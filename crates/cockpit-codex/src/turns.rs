//! How a Codex turn ended, from the app-server: the outcome of a failed turn, which sends no
//! hook (measured on 0.161.0), and the report the session launcher sends for it.
//!
//! A failed turn leaves its thread in the `systemError` status, which the app-server
//! announces to every connection (`thread/status/changed`), subscribed to the thread or not;
//! its turn then reads `failed`, with the error's kind (`codexErrorInfo`). An interrupted
//! or completed turn leaves the thread `idle`, and its hook reports it.

use cockpit_provider_api::{ConversationId, TurnOutcome};
use serde_json::{Value, json};

use crate::app_server::{AppServer, RpcError};
use crate::delivery::last_turn;

/// The agent channel's longest outcome reason, in characters.
const OUTCOME_REASON_MAX_CHARS: usize = 80;

/// The error kind that means the plan's usage limit stopped the turn.
const USAGE_LIMIT: &str = "usageLimitExceeded";

/// The outcome of `turn`, an app-server `Turn`, when it failed: `quota-stopped` when the
/// usage limit stopped it, otherwise `failed` with Codex's word for the error
/// (`unauthorized`, `responseStreamDisconnected`…). `None` for a turn that did not fail.
pub fn failure_outcome(turn: &Value) -> Option<TurnOutcome> {
    if turn.get("status").and_then(Value::as_str) != Some("failed") {
        return None;
    }
    let kind = turn
        .get("error")
        .and_then(|error| error.get("codexErrorInfo"))
        .and_then(|info| match info {
            Value::String(kind) => Some(kind.clone()),
            Value::Object(fields) => fields.keys().next().cloned(),
            _ => None,
        });
    if kind.as_deref() == Some(USAGE_LIMIT) {
        return Some(TurnOutcome::QuotaStopped);
    }
    Some(TurnOutcome::Failed { reason: kind })
}

/// `--outcome` and, for a failure whose reason the channel accepts (one plain word),
/// `--outcome-reason`. A reason it would refuse is left out, so the outcome still arrives.
pub fn outcome_arguments(outcome: &TurnOutcome) -> Vec<String> {
    let mut arguments = vec!["--outcome".to_owned(), outcome.name().to_owned()];
    let reason = outcome.failure_reason().filter(|reason| {
        reason.chars().count() <= OUTCOME_REASON_MAX_CHARS
            && !reason.is_empty()
            && reason
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
    });
    if let Some(reason) = reason {
        arguments.extend(["--outcome-reason".to_owned(), reason.to_owned()]);
    }
    arguments
}

/// The `cockpit` arguments that report `thread`'s turn ended with `outcome`.
pub fn outcome_report(thread: &ConversationId, outcome: &TurnOutcome) -> Vec<String> {
    let mut arguments: Vec<String> = ["agent", "report", "--state", "waiting"]
        .map(str::to_owned)
        .into();
    arguments.extend(outcome_arguments(outcome));
    arguments.extend(["--conversation".to_owned(), thread.as_str().to_owned()]);
    arguments
}

/// Follows `server`'s notifications and passes `report` the arguments of each failed turn
/// of a conversation thread; a helper thread's (ephemeral) or a subagent's is not the
/// conversation's turn. Returns when the connection ends, with why.
pub fn watch_failures(server: &mut AppServer, mut report: impl FnMut(Vec<String>)) -> RpcError {
    loop {
        let notification = match server.next_notification() {
            Ok(notification) => notification,
            Err(error) => return error,
        };
        let params = &notification.params;
        let is_system_error = notification.method == "thread/status/changed"
            && params.pointer("/status/type").and_then(Value::as_str) == Some("systemError");
        let thread = params.get("threadId").and_then(Value::as_str);
        let Some(thread) = thread.filter(|_| is_system_error) else {
            continue;
        };
        match failed_conversation_turn(server, thread) {
            Ok(Some((thread, outcome))) => report(outcome_report(&thread, &outcome)),
            Ok(None) => {}
            Err(error) => return error,
        }
    }
}

/// The thread and outcome when `thread` is a conversation whose last turn failed.
fn failed_conversation_turn(
    server: &mut AppServer,
    thread: &str,
) -> Result<Option<(ConversationId, TurnOutcome)>, RpcError> {
    let Ok(conversation) = ConversationId::new(thread) else {
        return Ok(None);
    };
    let read = server.call("thread/read", json!({"threadId": thread}))?;
    let is_ephemeral = read.pointer("/thread/ephemeral").and_then(Value::as_bool) == Some(true);
    let is_subagent = read
        .pointer("/thread/parentThreadId")
        .is_some_and(|parent| !parent.is_null());
    if is_ephemeral || is_subagent {
        return Ok(None);
    }
    let outcome = last_turn(server, thread)?
        .as_ref()
        .and_then(failure_outcome);
    Ok(outcome.map(|outcome| (conversation, outcome)))
}
