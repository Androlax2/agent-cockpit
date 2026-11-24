//! Qwen Code's hook events: normalized into the provider contract's vocabulary, and the
//! agent-channel report the in-sandbox bridge (`torlek-qwen-hook`) sends for each. Qwen Code's
//! hooks copy Claude Code's (event names, a JSON payload on standard input naming
//! `hook_event_name` and `session_id`, `SessionStart`'s `additionalContext`), read from its
//! `docs/users/features/hooks.md` and `packages/core/src/hooks/types.ts` at 0.28.4.

use std::fmt;

use cockpit_provider_api::{
    Activity, CompactionMoment, CompactionPhase, CompactionTrigger, ContextUsage, ConversationId,
    EventKind, ProviderEvent, TurnOutcome,
};

/// The hook events launches subscribe the bridge to. `SessionEnd` reports only a `/clear`:
/// the process ending is observed by `cockpit-processes`, not claimed by the agent.
pub const HOOK_EVENTS: [&str; 10] = [
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "PostToolUseFailure",
    "Notification",
    "Stop",
    "StopFailure",
    "PreCompact",
    "PostCompact",
    "SessionEnd",
];

/// The notification type that waits for the owner's answer: a tool's permission dialog.
/// `elicitation_dialog` is defined in 0.28.4 but never sent.
const PERMISSION_PROMPT: &str = "permission_prompt";

/// The agent channel's longest needs-input question, in characters.
const QUESTION_MAX_CHARS: usize = 300;
/// The agent channel's longest outcome reason, in characters.
const OUTCOME_REASON_MAX_CHARS: usize = 80;
/// Sent instead of a question the channel would refuse, so the state still arrives.
const PLAIN_QUESTION: &str = "Qwen Code is waiting for you in its terminal";

/// The `SessionEnd` reason after which the same agent goes on in another conversation:
/// `/clear`, whose `SessionStart` follows. An in-session `/resume` or `/branch` sends no
/// `SessionEnd` in 0.28.4, so the conversation it switches to is refused (#219).
const CONVERSATION_CHANGE: &str = "clear";

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
        "SessionStart" if text("source") == Some("compact") => EventKind::ContextCompacted,
        "SessionStart" => EventKind::Started {
            conversation: text("session_id").and_then(|id| ConversationId::new(id).ok()),
        },
        "PreCompact" => compaction(CompactionPhase::Started, text("trigger")),
        "PostCompact" => compaction(CompactionPhase::Ended, text("trigger")),
        "UserPromptSubmit" => EventKind::PromptSubmitted,
        "PostToolUse" | "PostToolUseFailure" => match text("tool_name") {
            Some(tool) => EventKind::ToolUsed {
                tool: tool.to_owned(),
            },
            None => EventKind::Unknown,
        },
        "Notification" => match text("notification_type") {
            Some(PERMISSION_PROMPT) => EventKind::NeedsYou {
                question: text("message").map(str::to_owned),
            },
            Some("idle_prompt") => EventKind::Idle,
            _ => EventKind::Unknown,
        },
        "Stop" => EventKind::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
        // Not `QuotaStopped`, even for `rate_limit`: Qwen Code derives that from any 429 or
        // "rate limit" message, a spent plan and a transient limit alike (#1446), and there
        // are no plan windows to tell them apart.
        "StopFailure" => EventKind::TurnEnded {
            outcome: TurnOutcome::Failed {
                reason: text("error").map(str::to_owned),
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

/// A compaction hook's moment; the trigger is `manual` or `auto`. Qwen Code's payload carries
/// no sizes.
fn compaction(phase: CompactionPhase, trigger: Option<&str>) -> EventKind {
    EventKind::Compaction(CompactionMoment {
        phase,
        trigger: trigger.and_then(CompactionTrigger::from_name),
        tokens_before: None,
        tokens_after: None,
    })
}

/// The `torlek` arguments that report `event` on the agent channel; `None` for an event that
/// proves no activity. A dialog's question goes as `--dialog`, which the next report with a
/// state clears; a submitted prompt adds `--prompt`, which clears the agent's own question
/// (#200).
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

/// The `torlek` arguments the bridge runs for `event`: its report ([`report_arguments`])
/// naming the conversation the payload comes from (`--conversation`), so the daemon refuses a
/// report of a conversation that is not the session's (#219), and at a turn's end the context
/// size the payload carries (`Stop`'s `input_tokens` and `context_limit`, #479). A `SessionEnd`
/// of `/clear` announces `--conversation-ends` instead, which lets the session follow the
/// conversation the next report names. `None` when nothing is reported; a payload that names
/// no valid conversation reports without one.
pub fn bridge_arguments(event: &ProviderEvent) -> Option<Vec<String>> {
    let payload: Option<serde_json::Value> = serde_json::from_str(&event.raw).ok();
    let field = |name: &str| payload.as_ref().and_then(|payload| payload.get(name));
    let text = |name: &str| field(name).and_then(serde_json::Value::as_str);
    let conversation = text("session_id")
        .and_then(|id| ConversationId::new(id).ok())
        .map(|id| ["--conversation".to_owned(), id.as_str().to_owned()]);
    if let EventKind::Compaction(moment) = &event.kind {
        let mut arguments = moment.report_arguments();
        arguments.extend(conversation.into_iter().flatten());
        return Some(arguments);
    }
    if event.kind == EventKind::Ended {
        let ending = conversation.filter(|_| text("reason") == Some(CONVERSATION_CHANGE))?;
        let mut arguments = vec!["agent".to_owned(), "report".to_owned()];
        arguments.extend(ending);
        arguments.push("--conversation-ends".to_owned());
        return Some(arguments);
    }
    let mut arguments = report_arguments(event)?;
    arguments.extend(conversation.into_iter().flatten());
    if matches!(event.kind, EventKind::TurnEnded { .. }) {
        let count = |name: &str| field(name).and_then(serde_json::Value::as_u64);
        if let Some(tokens) = count("input_tokens") {
            let usage = ContextUsage {
                tokens,
                window: count("context_limit").filter(|window| *window > 0),
            };
            arguments.extend(usage.report_arguments());
        }
    }
    Some(arguments)
}

/// `--outcome` and, for a failure with a reason the channel accepts, `--outcome-reason`. A
/// reason it would refuse is left out, so the outcome still arrives.
fn outcome_arguments(outcome: &TurnOutcome) -> Vec<String> {
    let mut arguments = vec!["--outcome".to_owned(), outcome.name().to_owned()];
    let reason = outcome
        .reason()
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

/// The `torlek` arguments that print the session's launch brief (#419).
pub const BRIEF_ARGUMENTS: [&str; 1] = ["brief"];

/// Whether `event` starts a context the launch brief must be in (#419): every
/// `SessionStart`, a new conversation, a resume, `/clear` or a compaction alike.
pub fn starts_context(event: &ProviderEvent) -> bool {
    event.name == "SessionStart"
}

/// The hook's answer that adds `brief` to the model's context: Qwen Code's `SessionStart`
/// `additionalContext`.
pub fn brief_context(brief: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": brief,
        }
    })
    .to_string()
}
