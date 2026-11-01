//! OpenCode's bus events: normalized into the provider contract's vocabulary, and the
//! agent-channel report the in-sandbox bridge (`cockpit-opencode-bridge`) sends for each.
//! The plugin (`plugin/cockpit-opencode-plugin.js`) hands the bridge one event at a time, as
//! its `event` hook receives it: `{"id", "type", "properties"}`.

use std::fmt;

use cockpit_provider_api::{Activity, ConversationId, EventKind, ProviderEvent, TurnOutcome};

/// The event types the plugin forwards, each only in the forms [`normalize`] maps to an
/// activity; the plugin's own `FORWARDED` list must match (a test compares them).
pub const FORWARDED_EVENTS: [&str; 6] = [
    "session.created",
    "session.status",
    "permission.asked",
    "question.asked",
    "message.part.updated",
    "message.updated",
];

/// Assistant `finish` reasons after which OpenCode's loop goes on: the turn is not over.
const UNFINISHED: [&str; 2] = ["tool-calls", "unknown"];

/// The error an assistant message ends with when the owner interrupts its turn.
const ABORTED: &str = "MessageAbortedError";

/// The agent channel's longest needs-input question, in characters.
const QUESTION_MAX_CHARS: usize = 300;
/// The agent channel's longest outcome reason, in characters.
const OUTCOME_REASON_MAX_CHARS: usize = 80;
/// Sent instead of a question the channel would refuse, so the state still arrives.
const PLAIN_QUESTION: &str = "OpenCode is waiting for you in its terminal";

/// An event that is not a JSON object naming its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventError {
    pub reason: String,
}

impl fmt::Display for EventError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for EventError {}

/// The event the plugin forwarded. An event, or a form of one, this adapter does not
/// normalize is [`EventKind::Unknown`], with the event kept raw.
pub fn normalize(event: &str) -> Result<ProviderEvent, EventError> {
    let value: serde_json::Value = serde_json::from_str(event).map_err(|error| EventError {
        reason: format!("the event is not JSON: {error}"),
    })?;
    let name = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| EventError {
            reason: "the event names no `type`".to_owned(),
        })?;
    let properties = value
        .get("properties")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let text = |pointer: &str| {
        properties
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
    };
    let kind = match name {
        // A subagent's session has a parent; its events are not this conversation's.
        "session.created" if text("/info/parentID").is_none() => EventKind::Started {
            conversation: text("/info/id").and_then(|id| ConversationId::new(id).ok()),
        },
        "session.status" => match text("/status/type") {
            Some("busy") => EventKind::PromptSubmitted,
            Some("idle") => EventKind::Idle,
            // `retry`: still in the turn, waiting to try the request again.
            _ => EventKind::Unknown,
        },
        "permission.asked" => EventKind::NeedsYou {
            question: text("/permission").map(|permission| {
                let patterns: Vec<&str> = properties
                    .get("patterns")
                    .and_then(serde_json::Value::as_array)
                    .map(|patterns| {
                        patterns
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect()
                    })
                    .unwrap_or_default();
                if patterns.is_empty() {
                    format!("OpenCode asks permission to {permission}")
                } else {
                    format!(
                        "OpenCode asks permission to {permission} {}",
                        patterns.join(", ")
                    )
                }
            }),
        },
        "question.asked" => EventKind::NeedsYou {
            question: text("/questions/0/question").map(str::to_owned),
        },
        "message.part.updated" => match (text("/part/type"), text("/part/state/status")) {
            (Some("tool"), Some("completed" | "error")) => match text("/part/tool") {
                Some(tool) => EventKind::ToolUsed {
                    tool: tool.to_owned(),
                },
                None => EventKind::Unknown,
            },
            _ => EventKind::Unknown,
        },
        "message.updated" => turn_end(&properties).unwrap_or(EventKind::Unknown),
        _ => EventKind::Unknown,
    };
    Ok(ProviderEvent {
        kind,
        name: name.to_owned(),
        raw: event.to_owned(),
    })
}

/// The end of a turn an assistant message announces once it is complete: failed with its
/// error's name, or completed when OpenCode's loop stops after it.
fn turn_end(properties: &serde_json::Value) -> Option<EventKind> {
    let info = properties.get("info")?;
    let text = |pointer: &str| info.pointer(pointer).and_then(serde_json::Value::as_str);
    if text("/role") != Some("assistant") || info.pointer("/time/completed")?.is_null() {
        return None;
    }
    // Not `QuotaStopped` for an `APIError`: a spent plan and a transient 429 look alike.
    if let Some(error) = info.get("error") {
        // The owner interrupted the turn: it did not fail, and it has no other outcome.
        if error.get("name").and_then(serde_json::Value::as_str) == Some(ABORTED) {
            return Some(EventKind::Idle);
        }
        return Some(EventKind::TurnEnded {
            outcome: TurnOutcome::Failed {
                reason: error
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            },
        });
    }
    let finish = text("/finish")?;
    (!UNFINISHED.contains(&finish)).then_some(EventKind::TurnEnded {
        outcome: TurnOutcome::Completed,
    })
}

/// The `cockpit` arguments that report `event` on the agent channel; `None` for an event
/// that proves no activity. A dialog's question goes as `--dialog`, which the next report
/// with a state clears; a turn's start adds `--prompt`, which clears the agent's own
/// question.
pub fn report_arguments(event: &ProviderEvent) -> Option<Vec<String>> {
    let activity = event.observation()?.activity;
    let mut arguments: Vec<String> = ["agent", "report", "--state"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    match activity {
        Activity::Working => {
            arguments.push("working".to_owned());
            if event.kind == EventKind::PromptSubmitted {
                arguments.push("--prompt".to_owned());
            }
        }
        Activity::Waiting => {
            arguments.push("waiting".to_owned());
            if let EventKind::TurnEnded { outcome } = &event.kind {
                arguments.extend(outcome_arguments(outcome));
            }
        }
        Activity::NeedsYou => {
            let question = match &event.kind {
                EventKind::NeedsYou { question } => question.as_deref(),
                _ => None,
            };
            arguments.push("waiting".to_owned());
            arguments.push("--dialog".to_owned());
            arguments.push(channel_question(question));
        }
    }
    Some(arguments)
}

/// The `cockpit` arguments the bridge runs for `event`: its report ([`report_arguments`])
/// naming the OpenCode session it comes from (`--conversation`), so the daemon refuses a
/// report of a session that is not this Cockpit session's, and names a new one by its first
/// report. `None` when nothing is reported; an event that names no valid session reports
/// without one.
pub fn bridge_arguments(event: &ProviderEvent) -> Option<Vec<String>> {
    let mut arguments = report_arguments(event)?;
    let value: Option<serde_json::Value> = serde_json::from_str(&event.raw).ok();
    let conversation = value
        .as_ref()
        .and_then(|value| value.pointer("/properties/sessionID"))
        .and_then(serde_json::Value::as_str)
        .and_then(|id| ConversationId::new(id).ok());
    if let Some(conversation) = conversation {
        arguments.extend([
            "--conversation".to_owned(),
            conversation.as_str().to_owned(),
        ]);
    }
    Some(arguments)
}

/// `--outcome` and, for a failure with a reason the channel accepts, `--outcome-reason`. A
/// reason it would refuse is left out, so the outcome still arrives.
fn outcome_arguments(outcome: &TurnOutcome) -> Vec<String> {
    let mut arguments = vec!["--outcome".to_owned(), outcome.name().to_owned()];
    let reason = outcome
        .failure_reason()
        .and_then(|reason| channel_text(reason, OUTCOME_REASON_MAX_CHARS));
    if let Some(reason) = reason {
        arguments.extend(["--outcome-reason".to_owned(), reason]);
    }
    arguments
}

/// `question` as the agent channel accepts it, otherwise a plain question.
fn channel_question(question: Option<&str>) -> String {
    question
        .and_then(|question| channel_text(question, QUESTION_MAX_CHARS))
        .unwrap_or_else(|| PLAIN_QUESTION.to_owned())
}

/// `text` as the agent channel accepts it: one line, at most `max_chars` characters (cut with
/// an ellipsis), without control, invisible formatting or tmux format sequences; `None` when
/// it has none of those to keep or is empty. Mirrors the channel's report validation, which
/// adapters cannot import (`provider adapters -> provider-api only`), as `cockpit-claude`
/// does; line breaks and tabs become spaces.
fn channel_text(text: &str, max_chars: usize) -> Option<String> {
    let is_refused = text.chars().any(|character| {
        (character.is_control() && !character.is_whitespace()) || is_invisible_format(character)
    }) || ["#(", "#{", "#["]
        .iter()
        .any(|opener| text.contains(opener));
    let words: Vec<&str> = text.split_whitespace().collect();
    if is_refused || words.is_empty() {
        return None;
    }
    let line = words.join(" ");
    if line.chars().count() <= max_chars {
        return Some(line);
    }
    let mut cut: String = line.chars().take(max_chars - 1).collect();
    cut.push('…');
    Some(cut)
}

/// The format characters the agent channel refuses: soft hyphen, zero-width marks,
/// bidirectional controls, word joiners and the byte-order mark.
fn is_invisible_format(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
    )
}
