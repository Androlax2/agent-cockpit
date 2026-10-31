//! Claude Code's hook events: normalized into the provider contract's vocabulary, and the
//! agent-channel report the in-sandbox bridge (`cockpit-claude-hook`) sends for each.

use std::fmt;

use cockpit_provider_api::{Activity, ConversationId, EventKind, ProviderEvent, TurnOutcome};

/// The hook events launches subscribe the bridge to. `SessionEnd` reports nothing: the
/// process ending is observed by `cockpit-processes`, not claimed by the agent.
pub const HOOK_EVENTS: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "Notification",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

/// Notification types that wait for the owner's answer.
const NEEDS_YOU_NOTIFICATIONS: [&str; 5] = [
    "permission_prompt",
    "worker_permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
    "agent_needs_input",
];

/// The agent channel's longest needs-input question, in characters.
const QUESTION_MAX_CHARS: usize = 300;
/// The agent channel's longest outcome reason, in characters.
const OUTCOME_REASON_MAX_CHARS: usize = 80;
/// Sent instead of a question the channel would refuse, so the state still arrives.
const PLAIN_QUESTION: &str = "Claude Code is waiting for you in its terminal";

/// A hook payload that is not a JSON object naming its event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookError {
    pub reason: String,
}

impl fmt::Display for HookError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for HookError {}

/// The event a hook payload (the hook's standard input) describes. An event this adapter
/// does not normalize is [`EventKind::Unknown`], with the payload kept raw.
pub fn normalize(payload: &str) -> Result<ProviderEvent, HookError> {
    let value: serde_json::Value = serde_json::from_str(payload).map_err(|error| HookError {
        reason: format!("the hook payload is not JSON: {error}"),
    })?;
    let name = value
        .get("hook_event_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| HookError {
            reason: "the hook payload names no `hook_event_name`".to_owned(),
        })?;
    let text = |field: &str| value.get(field).and_then(serde_json::Value::as_str);
    let kind = match name {
        // After a compaction, possibly automatic in the middle of a turn, it proves no wait.
        "SessionStart" if text("source") == Some("compact") => EventKind::Unknown,
        "SessionStart" => EventKind::Started {
            conversation: text("session_id").and_then(|id| ConversationId::new(id).ok()),
        },
        "UserPromptSubmit" => EventKind::PromptSubmitted,
        "PostToolUse" => match text("tool_name") {
            Some(tool) => EventKind::ToolUsed {
                tool: tool.to_owned(),
            },
            None => EventKind::Unknown,
        },
        "Notification" => match text("notification_type") {
            Some(kind) if NEEDS_YOU_NOTIFICATIONS.contains(&kind) => EventKind::NeedsYou {
                question: text("message").map(str::to_owned),
            },
            Some("idle_prompt") => EventKind::Idle,
            _ => EventKind::Unknown,
        },
        "Stop" => EventKind::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
        "StopFailure" => EventKind::TurnEnded {
            outcome: match text("error") {
                Some("rate_limit") => TurnOutcome::QuotaStopped,
                reason => TurnOutcome::Failed {
                    reason: reason.map(str::to_owned),
                },
            },
        },
        "SessionEnd" => EventKind::Ended,
        _ => EventKind::Unknown,
    };
    Ok(ProviderEvent {
        kind,
        name: name.to_owned(),
        raw: payload.to_owned(),
    })
}

/// The `cockpit` arguments that report `event` on the agent channel; `None` for an event
/// that proves no activity. A dialog's question goes as `--dialog`, which the next report
/// clears; a submitted prompt adds `--prompt`, which clears the agent's own question (#200).
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
/// adapters cannot import (`provider adapters -> provider-api only`); line breaks and tabs
/// become spaces.
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
