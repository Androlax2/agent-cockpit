//! Conformance with Claude Code's hook payloads, pinned to `CONFORMANCE_VERSION`: each event
//! normalized, and the agent-channel report the bridge sends for it.

mod common;

use cockpit_claude::hooks::{self, HOOK_EVENTS};
use cockpit_provider_api::{ConversationId, EventKind, TurnOutcome};

fn kind(fixture: &str) -> EventKind {
    hooks::normalize(&common::fixture(fixture)).unwrap().kind
}

fn report(fixture: &str) -> Option<Vec<String>> {
    hooks::report_arguments(&hooks::normalize(&common::fixture(fixture)).unwrap())
}

fn words(words: &[&str]) -> Option<Vec<String>> {
    Some(words.iter().map(|word| (*word).to_owned()).collect())
}

#[test]
fn session_start_names_the_conversation() {
    assert_eq!(
        kind("session-start"),
        EventKind::Started {
            conversation: Some(
                ConversationId::new("49d695c9-4342-47df-a3fc-9cec34484f36").unwrap()
            )
        }
    );
}

#[test]
fn each_pinned_payload_normalizes_to_its_event() {
    for (fixture, expected) in [
        ("user-prompt-submit", EventKind::PromptSubmitted),
        (
            "post-tool-use",
            EventKind::ToolUsed {
                tool: "Bash".to_owned(),
            },
        ),
        (
            "notification-permission-prompt",
            EventKind::NeedsYou {
                question: Some("Claude needs your permission".to_owned()),
            },
        ),
        ("notification-idle-prompt", EventKind::Idle),
        (
            "stop",
            EventKind::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
        ),
        (
            "stop-failure-rate-limit",
            EventKind::TurnEnded {
                outcome: TurnOutcome::QuotaStopped,
            },
        ),
        (
            "session-start-resume",
            EventKind::Started {
                conversation: Some(
                    ConversationId::new("576d7a7d-768a-47d6-ae5c-290c19e81fdc").unwrap(),
                ),
            },
        ),
        ("session-end", EventKind::Ended),
    ] {
        assert_eq!(kind(fixture), expected, "{fixture}");
    }
}

#[test]
fn the_raw_payload_and_its_event_name_are_kept() {
    let payload = common::fixture("stop");
    let event = hooks::normalize(&payload).unwrap();

    assert_eq!(
        (event.name.as_str(), event.raw.as_str()),
        ("Stop", payload.as_str())
    );
}

#[test]
fn an_event_the_adapter_does_not_know_is_kept_raw_as_unknown() {
    let payload = r#"{"hook_event_name":"PreCompact","trigger":"auto"}"#;
    let event = hooks::normalize(payload).unwrap();

    assert_eq!(event.kind, EventKind::Unknown);
    assert_eq!(event.name, "PreCompact");
    assert_eq!(event.raw, payload);
}

#[test]
fn a_notification_that_asks_nothing_is_unknown() {
    let payload = r#"{"hook_event_name":"Notification","message":"Signed in","notification_type":"auth_success"}"#;

    assert_eq!(hooks::normalize(payload).unwrap().kind, EventKind::Unknown);
}

#[test]
fn a_failure_other_than_the_usage_limit_keeps_its_reason() {
    let payload = r#"{"hook_event_name":"StopFailure","error":"server_error"}"#;

    assert_eq!(
        hooks::normalize(payload).unwrap().kind,
        EventKind::TurnEnded {
            outcome: TurnOutcome::Failed {
                reason: Some("server_error".to_owned())
            }
        }
    );
}

#[test]
fn a_failure_without_its_reason_says_none() {
    let payload = r#"{"hook_event_name":"StopFailure"}"#;

    assert_eq!(
        hooks::normalize(payload).unwrap().kind,
        EventKind::TurnEnded {
            outcome: TurnOutcome::Failed { reason: None }
        }
    );
}

/// An auto-compaction fires `SessionStart` in the middle of a turn: it proves no waiting.
#[test]
fn a_session_start_after_compaction_is_unknown() {
    assert_eq!(kind("session-start-compact"), EventKind::Unknown);
}

#[test]
fn a_tool_use_that_names_no_tool_is_unknown() {
    let payload = r#"{"hook_event_name":"PostToolUse","tool_input":{}}"#;

    assert_eq!(hooks::normalize(payload).unwrap().kind, EventKind::Unknown);
}

#[test]
fn a_payload_that_is_not_a_hook_event_is_refused() {
    for payload in ["", "[]", "{}", r#"{"hook_event_name":7}"#] {
        assert!(hooks::normalize(payload).is_err(), "{payload:?}");
    }
}

#[test]
fn each_event_reports_its_state_through_the_agent_channel() {
    for (fixture, expected) in [
        (
            "session-start",
            words(&["agent", "report", "--state", "waiting"]),
        ),
        (
            "user-prompt-submit",
            words(&["agent", "report", "--state", "working"]),
        ),
        (
            "post-tool-use",
            words(&["agent", "report", "--state", "working"]),
        ),
        (
            "notification-permission-prompt",
            words(&[
                "agent",
                "report",
                "--state",
                "waiting",
                "--needs-input",
                "Claude needs your permission",
            ]),
        ),
        (
            "notification-idle-prompt",
            words(&["agent", "report", "--state", "waiting"]),
        ),
        ("stop", words(&["agent", "report", "--state", "waiting"])),
        (
            "stop-failure-rate-limit",
            words(&["agent", "report", "--state", "waiting"]),
        ),
        ("session-end", None),
    ] {
        assert_eq!(report(fixture), expected, "{fixture}");
    }
}

#[test]
fn a_question_the_channel_would_refuse_is_replaced_by_a_plain_one() {
    for message in ["Run #(rm -rf ~)?", "bidi \u{202E}txet", "a\u{0007}bell", ""] {
        let payload = serde_json::json!({
            "hook_event_name": "Notification",
            "notification_type": "permission_prompt",
            "message": message,
        })
        .to_string();
        let arguments = hooks::report_arguments(&hooks::normalize(&payload).unwrap()).unwrap();

        assert_eq!(
            arguments.last().unwrap(),
            "Claude Code is waiting for you in its terminal",
            "{message:?}"
        );
    }
}

#[test]
fn a_multi_line_question_is_joined_onto_one_line() {
    let payload = serde_json::json!({
        "hook_event_name": "Notification",
        "notification_type": "elicitation_dialog",
        "message": "Pick one:\n\tmain or\r\ndevelop",
    })
    .to_string();
    let arguments = hooks::report_arguments(&hooks::normalize(&payload).unwrap()).unwrap();

    assert_eq!(arguments.last().unwrap(), "Pick one: main or develop");
}

#[test]
fn a_long_question_is_cut_to_the_channels_limit() {
    let payload = serde_json::json!({
        "hook_event_name": "Notification",
        "notification_type": "permission_prompt",
        "message": "word ".repeat(100),
    })
    .to_string();
    let arguments = hooks::report_arguments(&hooks::normalize(&payload).unwrap()).unwrap();
    let question = arguments.last().unwrap();

    assert_eq!(question.chars().count(), 300);
    assert!(question.ends_with('…'));
}

#[test]
fn every_subscribed_hook_event_has_a_pinned_payload() {
    let fixtures: Vec<String> = std::fs::read_dir(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "tests/fixtures/claude-code-{}",
            cockpit_claude::CONFORMANCE_VERSION
        )),
    )
    .unwrap()
    .filter_map(|entry| {
        let text = std::fs::read_to_string(entry.unwrap().path()).ok()?;
        let payload: serde_json::Value = serde_json::from_str(&text).ok()?;
        Some(payload["hook_event_name"].as_str()?.to_owned())
    })
    .collect();

    for event in HOOK_EVENTS {
        assert!(fixtures.iter().any(|name| name == event), "{event}");
    }
}
