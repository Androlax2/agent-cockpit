//! Claude Code's hook events: normalized into the provider contract's vocabulary, and the
//! agent-channel report the in-sandbox bridge (`cockpit-claude-hook`) sends for each.

use std::fmt;

use cockpit_provider_api::{ConversationId, EventKind, ObservedBy, ProviderEvent, TurnOutcome};

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
        "SessionStart" => EventKind::Started {
            conversation: text("session_id").and_then(|id| ConversationId::new(id).ok()),
        },
        "UserPromptSubmit" => EventKind::PromptSubmitted,
        "PostToolUse" => EventKind::ToolUsed {
            tool: text("tool_name").unwrap_or_default().to_owned(),
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
                    reason: reason.unwrap_or("unknown").to_owned(),
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
/// that proves no activity.
pub fn report_arguments(event: &ProviderEvent) -> Option<Vec<String>> {
    let observation = event.observation()?;
    debug_assert_eq!(observation.observed_by, ObservedBy::Event);
    let mut arguments: Vec<String> = ["agent", "report", "--state"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    match &event.kind {
        EventKind::NeedsYou { question } => {
            arguments.push("waiting".to_owned());
            arguments.push("--needs-input".to_owned());
            arguments.push(channel_question(question.as_deref()));
        }
        EventKind::PromptSubmitted | EventKind::ToolUsed { .. } => {
            arguments.push("working".to_owned());
        }
        _ => arguments.push("waiting".to_owned()),
    }
    Some(arguments)
}

/// `question` as the agent channel accepts it: one line, at most 300 characters, without
/// control, invisible formatting or tmux format sequences; otherwise a plain question.
fn channel_question(question: Option<&str>) -> String {
    let Some(question) = question else {
        return PLAIN_QUESTION.to_owned();
    };
    let is_refused = question
        .chars()
        .any(|character| character.is_control() || is_invisible_format(character))
        || ["#(", "#{", "#["]
            .iter()
            .any(|opener| question.contains(opener));
    let words: Vec<&str> = question.split_whitespace().collect();
    if is_refused || words.is_empty() {
        return PLAIN_QUESTION.to_owned();
    }
    let line = words.join(" ");
    if line.chars().count() <= QUESTION_MAX_CHARS {
        return line;
    }
    let mut cut: String = line.chars().take(QUESTION_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
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
