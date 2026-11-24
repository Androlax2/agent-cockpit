//! Qwen Code's hook payloads, normalized and turned into agent-channel reports.

mod common;

use cockpit_provider_api::{
    Activity, CompactionPhase, CompactionTrigger, ConversationId, EventKind, TurnOutcome,
};
use cockpit_qwen::hooks;

fn kind(fixture: &str) -> EventKind {
    hooks::normalize(&common::fixture(fixture)).unwrap().kind
}

fn bridge_arguments(fixture: &str) -> Option<Vec<String>> {
    hooks::bridge_arguments(&hooks::normalize(&common::fixture(fixture)).unwrap())
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

fn with_conversation(report: &str) -> Vec<String> {
    let mut arguments = words(report);
    arguments.extend([
        "--conversation".to_owned(),
        common::FIXTURE_SESSION.to_owned(),
    ]);
    arguments
}

#[test]
fn each_payload_normalizes_to_its_event() {
    let session = ConversationId::new(common::FIXTURE_SESSION).unwrap();
    let cases = [
        (
            "session-start-startup",
            EventKind::Started {
                conversation: Some(session.clone()),
            },
        ),
        (
            "session-start-resume",
            EventKind::Started {
                conversation: Some(session.clone()),
            },
        ),
        (
            "session-start-clear",
            EventKind::Started {
                conversation: Some(session),
            },
        ),
        ("session-start-compact", EventKind::ContextCompacted),
        ("user-prompt-submit", EventKind::PromptSubmitted),
        (
            "post-tool-use",
            EventKind::ToolUsed {
                tool: "run_shell_command".to_owned(),
            },
        ),
        (
            "post-tool-use-failure",
            EventKind::ToolUsed {
                tool: "edit".to_owned(),
            },
        ),
        (
            "notification-permission-prompt",
            EventKind::NeedsYou {
                question: Some(
                    "Qwen Code needs your permission to use run_shell_command".to_owned(),
                ),
            },
        ),
        ("notification-idle-prompt", EventKind::Idle),
        ("notification-auth-success", EventKind::Unknown),
        (
            "stop",
            EventKind::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
        ),
        (
            "stop-failure-server-error",
            EventKind::TurnEnded {
                outcome: TurnOutcome::Failed {
                    reason: Some("server_error".to_owned()),
                },
            },
        ),
        ("session-end-exit", EventKind::Ended),
    ];
    for (fixture, expected) in cases {
        assert_eq!(kind(fixture), expected, "{fixture}");
    }
}

/// Qwen Code sends `rate_limit` for any 429, a spent plan and a transient limit alike.
#[test]
fn a_rate_limit_is_a_failed_turn_never_a_quota_stop() {
    assert_eq!(
        kind("stop-failure-rate-limit"),
        EventKind::TurnEnded {
            outcome: TurnOutcome::Failed {
                reason: Some("rate_limit".to_owned()),
            },
        }
    );
}

#[test]
fn compaction_hooks_are_its_moments_with_their_trigger() {
    for (fixture, phase, trigger) in [
        (
            "pre-compact-manual",
            CompactionPhase::Started,
            CompactionTrigger::Manual,
        ),
        (
            "post-compact-auto",
            CompactionPhase::Ended,
            CompactionTrigger::Auto,
        ),
    ] {
        let EventKind::Compaction(moment) = kind(fixture) else {
            panic!("{fixture} is not a compaction");
        };
        assert_eq!((moment.phase, moment.trigger), (phase, Some(trigger)));
    }
}

#[test]
fn a_payload_that_is_not_json_or_names_no_event_is_refused() {
    for payload in ["not json", r#"{"session_id":"x"}"#] {
        assert!(hooks::normalize(payload).is_err(), "{payload}");
    }
}

#[test]
fn each_report_names_the_payload_s_conversation() {
    let cases = [
        ("session-start-startup", "agent report --state waiting"),
        (
            "user-prompt-submit",
            "agent report --state working --prompt",
        ),
        ("post-tool-use", "agent report --state working"),
        ("notification-idle-prompt", "agent report --state waiting"),
        (
            "stop-failure-server-error",
            "agent report --state waiting --outcome failed --outcome-reason server_error",
        ),
    ];
    for (fixture, report) in cases {
        assert_eq!(
            bridge_arguments(fixture),
            Some(with_conversation(report)),
            "{fixture}"
        );
    }
}

#[test]
fn a_permission_prompt_reports_its_message_as_the_dialog() {
    let mut expected = words("agent report --state waiting --dialog");
    expected.push("Qwen Code needs your permission to use run_shell_command".to_owned());
    expected.extend([
        "--conversation".to_owned(),
        common::FIXTURE_SESSION.to_owned(),
    ]);

    assert_eq!(
        bridge_arguments("notification-permission-prompt"),
        Some(expected)
    );
}

/// #479: Qwen Code puts the turn's context size in the `Stop` payload itself.
#[test]
fn a_turn_s_end_reports_its_context_tokens_and_window() {
    let mut expected = with_conversation("agent report --state waiting --outcome completed");
    expected.extend(words("--context-tokens 41200 --context-window 1000000"));

    assert_eq!(bridge_arguments("stop"), Some(expected));
}

#[test]
fn a_turn_s_end_without_usage_reports_without_a_size() {
    assert_eq!(
        bridge_arguments("stop-without-usage"),
        Some(with_conversation(
            "agent report --state waiting --outcome completed"
        ))
    );
}

#[test]
fn a_compaction_reports_its_moment_for_the_conversation() {
    assert_eq!(
        bridge_arguments("pre-compact-manual"),
        Some(with_conversation(
            "agent report --compaction started --compaction-trigger manual"
        ))
    );
}

/// #219: the session follows the conversation `/clear` starts.
#[test]
fn a_clear_announces_that_its_conversation_ends() {
    assert_eq!(
        bridge_arguments("session-end-clear"),
        Some(words(&format!(
            "agent report --conversation {} --conversation-ends",
            common::FIXTURE_SESSION
        )))
    );
}

#[test]
fn an_exit_or_an_unknown_notification_reports_nothing() {
    for fixture in [
        "session-end-exit",
        "notification-auth-success",
        "session-start-compact",
    ] {
        assert_eq!(bridge_arguments(fixture), None, "{fixture}");
    }
}

#[test]
fn a_question_the_channel_would_refuse_becomes_a_plain_one() {
    let payload = r#"{"hook_event_name":"Notification","notification_type":"permission_prompt","message":"run #(rm -rf ~)?"}"#;
    let arguments = hooks::bridge_arguments(&hooks::normalize(payload).unwrap()).unwrap();

    assert_eq!(
        arguments.last().map(String::as_str),
        Some("Qwen Code is waiting for you in its terminal")
    );
}

#[test]
fn every_session_start_carries_the_brief_and_nothing_else_does() {
    for fixture in [
        "session-start-startup",
        "session-start-resume",
        "session-start-clear",
        "session-start-compact",
    ] {
        let event = hooks::normalize(&common::fixture(fixture)).unwrap();
        assert!(hooks::starts_context(&event), "{fixture}");
    }
    let stop = hooks::normalize(&common::fixture("stop")).unwrap();
    assert!(!hooks::starts_context(&stop));
}

#[test]
fn the_brief_is_session_start_s_additional_context() {
    let answer: serde_json::Value =
        serde_json::from_str(&hooks::brief_context("# Torlek launch brief")).unwrap();

    assert_eq!(
        answer,
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": "# Torlek launch brief",
            }
        })
    );
}

#[test]
fn a_session_start_observes_a_wait() {
    let event = hooks::normalize(&common::fixture("session-start-startup")).unwrap();

    assert_eq!(event.observation().unwrap().activity, Activity::Waiting);
}
