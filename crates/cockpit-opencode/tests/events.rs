//! Conformance with the events pinned in `tests/fixtures/opencode-1.19.5/`, and the agent
//! channel reports they become.

mod common;

use cockpit_opencode::events::{self, FORWARDED_EVENTS};
use cockpit_provider_api::{ConversationId, EventKind, TurnOutcome};

const SESSION: &str = "ses_1c4f2e8a3ffeJ7qkR2mWbXzD0v";

fn kind(fixture: &str) -> EventKind {
    events::normalize(&common::fixture(fixture)).unwrap().kind
}

fn report(fixture: &str) -> Option<Vec<String>> {
    events::bridge_arguments(&events::normalize(&common::fixture(fixture)).unwrap())
}

fn words(line: &str) -> Vec<String> {
    line.split(' ').map(str::to_owned).collect()
}

#[test]
fn each_pinned_event_normalizes_to_its_kind() {
    let failed = |reason: &str| EventKind::TurnEnded {
        outcome: TurnOutcome::Failed {
            reason: Some(reason.to_owned()),
        },
    };
    for (fixture, expected) in [
        (
            "session-created",
            EventKind::Started {
                conversation: Some(ConversationId::new(SESSION).unwrap()),
            },
        ),
        ("session-created-subagent", EventKind::Unknown),
        ("session-status-busy", EventKind::PromptSubmitted),
        ("session-status-retry", EventKind::Unknown),
        ("session-status-idle", EventKind::Idle),
        (
            "permission-asked",
            EventKind::NeedsYou {
                question: Some("OpenCode asks permission to bash git push origin main".to_owned()),
            },
        ),
        (
            "question-asked",
            EventKind::NeedsYou {
                question: Some("Which database should the tests use?".to_owned()),
            },
        ),
        (
            "tool-completed",
            EventKind::ToolUsed {
                tool: "bash".to_owned(),
            },
        ),
        (
            "tool-error",
            EventKind::ToolUsed {
                tool: "edit".to_owned(),
            },
        ),
        ("tool-running", EventKind::Unknown),
        (
            "message-assistant-stop",
            EventKind::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
        ),
        ("message-assistant-tool-calls", EventKind::Unknown),
        ("message-assistant-aborted", EventKind::Idle),
        ("message-assistant-api-error", failed("APIError")),
        ("message-assistant-streaming", EventKind::Unknown),
        ("session-idle", EventKind::Unknown),
    ] {
        assert_eq!(kind(fixture), expected, "{fixture}");
    }
}

#[test]
fn an_event_keeps_its_raw_form_and_its_name() {
    let raw = common::fixture("session-status-retry");

    let event = events::normalize(&raw).unwrap();

    assert_eq!(event.name, "session.status");
    assert_eq!(event.raw, raw);
}

#[test]
fn each_event_reports_its_state_from_its_session() {
    for (fixture, expected) in [
        ("session-created", "agent report --state waiting"),
        (
            "session-status-busy",
            "agent report --state working --prompt",
        ),
        ("session-status-idle", "agent report --state waiting"),
        ("tool-completed", "agent report --state working"),
        (
            "message-assistant-stop",
            "agent report --state waiting --outcome completed",
        ),
        ("message-assistant-aborted", "agent report --state waiting"),
    ] {
        let mut expected = words(expected);
        expected.extend(["--conversation".to_owned(), SESSION.to_owned()]);
        assert_eq!(report(fixture), Some(expected), "{fixture}");
    }
}

#[test]
fn a_dialog_reports_its_question() {
    let report = report("permission-asked").unwrap();

    assert_eq!(
        report,
        [
            "agent",
            "report",
            "--state",
            "waiting",
            "--dialog",
            "OpenCode asks permission to bash git push origin main",
            "--conversation",
            SESSION,
        ]
    );
}

#[test]
fn an_api_error_is_a_failure_never_a_quota_stop() {
    let report = report("message-assistant-api-error").unwrap();

    assert!(report.contains(&"failed".to_owned()), "{report:?}");
    assert!(!report.contains(&"quota-stopped".to_owned()), "{report:?}");
}

#[test]
fn what_proves_no_activity_reports_nothing() {
    for fixture in [
        "session-created-subagent",
        "session-status-retry",
        "tool-running",
        "message-assistant-tool-calls",
        "message-assistant-streaming",
        "session-idle",
    ] {
        assert_eq!(report(fixture), None, "{fixture}");
    }
}

#[test]
fn a_question_the_channel_would_refuse_becomes_a_plain_one() {
    let event = serde_json::json!({
        "id": "evt_1",
        "type": "question.asked",
        "properties": {
            "sessionID": SESSION,
            "questions": [{ "question": "Run #(rm -rf ~)?" }],
        },
    });

    let report = events::bridge_arguments(&events::normalize(&event.to_string()).unwrap());

    assert_eq!(
        report.unwrap()[5],
        "OpenCode is waiting for you in its terminal"
    );
}

#[test]
fn a_long_question_is_cut_to_300_characters_on_one_line() {
    let question = format!("line one\n{}", "x".repeat(400));
    let event = serde_json::json!({
        "id": "evt_1",
        "type": "question.asked",
        "properties": { "sessionID": SESSION, "questions": [{ "question": question }] },
    });

    let report = events::bridge_arguments(&events::normalize(&event.to_string()).unwrap());
    let asked = &report.unwrap()[5];

    assert_eq!(asked.chars().count(), 300);
    assert!(asked.starts_with("line one x"));
    assert!(asked.ends_with('…'));
}

#[test]
fn an_event_without_a_valid_session_reports_without_one() {
    let event = r#"{"id":"evt_1","type":"session.status","properties":{"sessionID":"has space","status":{"type":"idle"}}}"#;

    let report = events::bridge_arguments(&events::normalize(event).unwrap()).unwrap();

    assert_eq!(report, words("agent report --state waiting"));
}

#[test]
fn an_event_that_is_not_json_or_has_no_type_is_refused() {
    for event in ["not json", "{}", r#"{"type":3}"#] {
        assert!(events::normalize(event).is_err(), "{event:?}");
    }
}

#[test]
fn every_forwarded_event_type_has_a_pinned_event() {
    let fixtures: Vec<String> = std::fs::read_dir(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/opencode-1.19.5"
    ))
    .unwrap()
    .filter_map(|entry| {
        let name = entry.unwrap().file_name().into_string().unwrap();
        name.strip_suffix(".json").map(str::to_owned)
    })
    .map(|name| events::normalize(&common::fixture(&name)).unwrap().name)
    .collect();

    for forwarded in FORWARDED_EVENTS {
        assert!(fixtures.iter().any(|name| name == forwarded), "{forwarded}");
    }
}
