//! Conformance with Claude Code's hook payloads, pinned to `CONFORMANCE_VERSION`: each event
//! normalized, and the agent-channel report the bridge sends for it.

mod common;

use cockpit_claude::hooks::{self, HOOK_EVENTS};
use cockpit_provider_api::{
    CompactionMoment, CompactionPhase, CompactionTrigger, ConversationId, EventKind, TurnOutcome,
};

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
            "stop-failure-rate-limit-plan-window",
            EventKind::TurnEnded {
                outcome: TurnOutcome::Failed {
                    reason: Some("rate_limit".to_owned()),
                },
            },
        ),
        (
            "stop-failure-rate-limit-transient",
            EventKind::TurnEnded {
                outcome: TurnOutcome::Failed {
                    reason: Some("rate_limit".to_owned()),
                },
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
    let payload = r#"{"hook_event_name":"PreToolUse","tool_name":"Bash"}"#;
    let event = hooks::normalize(payload).unwrap();

    assert_eq!(event.kind, EventKind::Unknown);
    assert_eq!(event.name, "PreToolUse");
    assert_eq!(event.raw, payload);
}

#[test]
fn a_notification_that_asks_nothing_is_unknown() {
    let payload = r#"{"hook_event_name":"Notification","message":"Signed in","notification_type":"auth_success"}"#;

    assert_eq!(hooks::normalize(payload).unwrap().kind, EventKind::Unknown);
}

#[test]
fn a_failure_keeps_its_reason() {
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

/// An auto-compaction fires `SessionStart` in the middle of a turn: it proves no waiting, and
/// it is a known event of the compaction, not an unknown one (#782).
#[test]
fn a_session_start_after_compaction_is_a_known_event_that_proves_no_wait() {
    let event = hooks::normalize(&common::fixture("session-start-compact")).unwrap();

    assert_eq!(event.kind, EventKind::ContextCompacted);
    assert_eq!(event.observation(), None);
    assert_eq!(hooks::bridge_arguments(&event), None);
    assert!(hooks::starts_context(&event), "the brief must still return");
}

#[test]
fn a_pre_compact_starts_a_compaction_with_its_trigger() {
    assert_eq!(
        kind("pre-compact"),
        EventKind::Compaction(CompactionMoment {
            phase: CompactionPhase::Started,
            trigger: Some(CompactionTrigger::Auto),
            tokens_before: None,
            tokens_after: None,
        })
    );
}

#[test]
fn a_post_compact_ends_a_compaction_with_its_trigger() {
    assert_eq!(
        kind("post-compact"),
        EventKind::Compaction(CompactionMoment {
            phase: CompactionPhase::Ended,
            trigger: Some(CompactionTrigger::Manual),
            tokens_before: None,
            tokens_after: None,
        })
    );
}

#[test]
fn a_compaction_reports_itself_and_no_state() {
    let event = hooks::normalize(&common::fixture("pre-compact")).unwrap();

    assert_eq!(
        hooks::bridge_arguments(&event),
        words(&[
            "agent",
            "report",
            "--compaction",
            "started",
            "--compaction-trigger",
            "auto",
            "--conversation",
            "49d695c9-4342-47df-a3fc-9cec34484f36",
        ])
    );
}

/// The transcript holds the sizes: the last main-chain model call before the compaction, and
/// the boundary record's `postTokens` after it.
#[test]
fn a_compaction_reports_the_context_sizes_the_transcript_holds() {
    let directory = tempfile::tempdir().unwrap();
    let transcript = directory.path().join("transcript.jsonl");
    std::fs::copy(common::transcript("transcript-compaction"), &transcript).unwrap();
    let with_transcript = |fixture: &str| {
        let mut payload: serde_json::Value =
            serde_json::from_str(&common::fixture(fixture)).unwrap();
        payload["transcript_path"] = transcript.to_str().unwrap().into();
        hooks::normalize(&payload.to_string()).unwrap()
    };

    let before = hooks::measured_arguments(&with_transcript("pre-compact")).unwrap();
    let after = hooks::measured_arguments(&with_transcript("post-compact")).unwrap();

    assert!(
        before
            .windows(2)
            .any(|pair| pair == ["--compaction-tokens-before", "268012"]),
        "{before:?}"
    );
    assert!(
        after
            .windows(2)
            .any(|pair| pair == ["--compaction-tokens-after", "23678"]),
        "{after:?}"
    );
}

#[test]
fn a_compaction_whose_transcript_cannot_be_read_reports_without_sizes() {
    let event = hooks::normalize(&common::fixture("pre-compact")).unwrap();

    assert_eq!(
        hooks::measured_arguments(&event),
        hooks::bridge_arguments(&event)
    );
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
            words(&["agent", "report", "--state", "working", "--prompt"]),
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
                "--dialog",
                "Claude needs your permission",
            ]),
        ),
        (
            "notification-idle-prompt",
            words(&["agent", "report", "--state", "waiting"]),
        ),
        (
            "stop",
            words(&[
                "agent",
                "report",
                "--state",
                "waiting",
                "--outcome",
                "completed",
            ]),
        ),
        (
            "stop-failure-rate-limit-plan-window",
            words(&[
                "agent",
                "report",
                "--state",
                "waiting",
                "--outcome",
                "failed",
                "--outcome-reason",
                "rate_limit",
            ]),
        ),
        (
            "stop-failure-rate-limit-transient",
            words(&[
                "agent",
                "report",
                "--state",
                "waiting",
                "--outcome",
                "failed",
                "--outcome-reason",
                "rate_limit",
            ]),
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
