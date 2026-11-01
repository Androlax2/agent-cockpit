//! Codex's hook events: the subscription a launch passes on its command line, with the trust
//! Codex requires for it, the events normalized into the provider contract's vocabulary, and
//! the agent-channel report the in-sandbox bridge (`cockpit-codex-hook`) sends for each.

use std::fmt;

use cockpit_provider_api::{Activity, ConversationId, EventKind, ProviderEvent};
use sha2::{Digest, Sha256};

/// One hook event a launch subscribes the bridge to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookEvent {
    /// The event's name in the hook configuration and in `hook_event_name`.
    pub name: &'static str,
    /// Codex's snake-case label for it, part of the hook's trust key and hash.
    pub label: &'static str,
    /// Seconds Codex lets the hook run; it blocks the agent meanwhile.
    pub timeout_seconds: u64,
}

/// The agent channel's report deadline: a report that has not answered by then has failed.
const REPORT_SECONDS: u64 = 10;

/// The hook events launches subscribe the bridge to. `Interrupt` is capped at 3 seconds by
/// Codex. `SessionEnd` is not subscribed: it fires only when the thread shuts down, which
/// `cockpit-processes` observes, not when `/clear` leaves it (measured on 0.161.0).
pub const HOOK_EVENTS: [HookEvent; 6] = [
    HookEvent {
        name: "SessionStart",
        label: "session_start",
        timeout_seconds: REPORT_SECONDS,
    },
    HookEvent {
        name: "UserPromptSubmit",
        label: "user_prompt_submit",
        timeout_seconds: REPORT_SECONDS,
    },
    HookEvent {
        name: "PostToolUse",
        label: "post_tool_use",
        timeout_seconds: REPORT_SECONDS,
    },
    HookEvent {
        name: "PermissionRequest",
        label: "permission_request",
        timeout_seconds: REPORT_SECONDS,
    },
    HookEvent {
        name: "Stop",
        label: "stop",
        timeout_seconds: REPORT_SECONDS,
    },
    HookEvent {
        name: "Interrupt",
        label: "interrupt",
        timeout_seconds: 3,
    },
];

/// Where Codex files hooks given with `-c`: its key for a hook is this path, the event's
/// label, the matcher group's index and the handler's index.
const SESSION_FLAGS_SOURCE: &str = "/<session-flags>/config.toml";

/// Sent as the question of a permission request: its tool input is the agent's text, never
/// shown as Cockpit's.
const PLAIN_QUESTION: &str = "Codex is waiting for you in its terminal";

/// The `-c` values that subscribe `command` to every [`HOOK_EVENTS`] event for this launch
/// only, and trust exactly those hooks (`hooks.state`), so Codex runs them without the
/// owner's persisted trust and without `--dangerously-bypass-hook-trust`, which would also
/// run every untrusted hook of the project. A hook whose command differs from the trusted
/// one is not run.
pub fn subscription(command: &str) -> Vec<String> {
    let mut values: Vec<String> = HOOK_EVENTS
        .iter()
        .map(|event| {
            format!(
                "hooks.{}=[{{hooks=[{{type=\"command\",command={},timeout={}}}]}}]",
                event.name,
                toml_string(command),
                event.timeout_seconds
            )
        })
        .collect();
    let trusted: Vec<String> = HOOK_EVENTS
        .iter()
        .map(|event| {
            format!(
                "{}={{trusted_hash={}}}",
                toml_string(&trust_key(event)),
                toml_string(&trusted_hash(event, command))
            )
        })
        .collect();
    values.push(format!("hooks.state={{{}}}", trusted.join(",")));
    values
}

/// Codex's key for the one hook a launch subscribes to `event`.
pub fn trust_key(event: &HookEvent) -> String {
    format!("{SESSION_FLAGS_SOURCE}:{}:0:0", event.label)
}

/// The hash Codex 0.161.0 gives a command hook on `event`: SHA-256 of its normalized
/// identity as JSON with sorted keys and no spaces (`version_for_toml` over
/// `NormalizedHookIdentity` in `codex-rs/hooks`).
/// `conformance::codex_trusts_every_hook_a_launch_subscribes` checks it against the installed
/// Codex.
pub fn trusted_hash(event: &HookEvent, command: &str) -> String {
    let identity = format!(
        "{{\"event_name\":{},\"hooks\":[{{\"async\":false,\"command\":{},\"timeout\":{},\"type\":\"command\"}}]}}",
        json_string(event.label),
        json_string(command),
        event.timeout_seconds
    );
    let digest = Sha256::digest(identity.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

fn json_string(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

/// `text` as a TOML basic string.
fn toml_string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            character if character.is_control() => {
                quoted.push_str(&format!("\\u{:04X}", u32::from(character)));
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

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
/// does not normalize is [`EventKind::Unknown`], with the payload kept raw. So is a
/// subagent's (`agent_id`) activity: it is not the conversation's turn. A subagent's
/// permission request still waits for the owner, so it needs you like the agent's own.
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
    let is_subagent = value.get("agent_id").is_some();
    let kind = match name {
        "PermissionRequest" => EventKind::NeedsYou { question: None },
        _ if is_subagent => EventKind::Unknown,
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
        // Codex sends `Stop` only for a turn that completed; a failed one sends no hook.
        "Stop" => EventKind::TurnEnded {
            outcome: cockpit_provider_api::TurnOutcome::Completed,
        },
        // The owner interrupted the turn; Codex waits at its prompt.
        "Interrupt" => EventKind::Idle,
        "SessionEnd" => EventKind::Ended,
        _ => EventKind::Unknown,
    };
    Ok(ProviderEvent {
        kind,
        name: name.to_owned(),
        raw: payload.to_owned(),
    })
}

/// The `cockpit` arguments the bridge runs for `event`, naming the conversation the payload
/// comes from (`--conversation`, Codex's thread id) so the daemon refuses a report of
/// another conversation and names the session's conversation from its first report. A
/// permission request goes as `--dialog`, which the next report with a state clears; a
/// submitted prompt adds `--prompt`, which clears the agent's own question (#200). A turn's
/// end reports its `--outcome`; a failed turn sends no hook, and the session launcher
/// reports it ([`crate::turns`]). `None` when nothing is reported; a payload that names no
/// valid conversation reports without one.
pub fn bridge_arguments(event: &ProviderEvent) -> Option<Vec<String>> {
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
                arguments.extend(crate::turns::outcome_arguments(outcome));
            }
        }
        Activity::NeedsYou => {
            arguments.extend(["waiting", "--dialog", PLAIN_QUESTION].map(str::to_owned));
        }
    }
    let payload: Option<serde_json::Value> = serde_json::from_str(&event.raw).ok();
    let conversation = payload
        .as_ref()
        .and_then(|payload| payload.get("session_id"))
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
