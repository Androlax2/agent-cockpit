//! The conversation each bridge report comes from (#219): every report names the payload's
//! `session_id`, and a `/clear` or an in-session `/resume` announces that the conversation
//! ends for another, so the daemon can tell a legitimate change from a report of another
//! conversation. Driven with the payloads pinned from Claude Code 2.1.316.

mod common;

use cockpit_claude::hooks;

fn bridge(fixture: &str) -> Option<Vec<String>> {
    hooks::bridge_arguments(&hooks::normalize(&common::fixture(fixture)).unwrap())
}

fn words(words: &[&str]) -> Option<Vec<String>> {
    Some(words.iter().map(|word| (*word).to_owned()).collect())
}

#[test]
fn a_report_names_the_conversation_its_payload_comes_from() {
    assert_eq!(
        bridge("stop"),
        words(&[
            "agent",
            "report",
            "--state",
            "waiting",
            "--outcome",
            "completed",
            "--conversation",
            "49d695c9-4342-47df-a3fc-9cec34484f36",
        ])
    );
}

#[test]
fn a_clear_or_an_in_session_resume_announces_that_the_conversation_ends() {
    for (fixture, ending) in [
        ("session-end-clear", "003eb9a6-45b4-40b6-9555-20f4c6c02655"),
        ("session-end-resume", "e0cd85ac-35c1-414a-9749-fd79e69843b6"),
    ] {
        assert_eq!(
            bridge(fixture),
            words(&[
                "agent",
                "report",
                "--conversation",
                ending,
                "--conversation-ends"
            ]),
            "{fixture}"
        );
    }
}

#[test]
fn the_next_conversation_is_named_by_its_start() {
    for (fixture, next) in [
        (
            "session-start-clear",
            "e0cd85ac-35c1-414a-9749-fd79e69843b6",
        ),
        (
            "session-start-resume-switch",
            "b8a100bf-8448-49e3-bf1d-424d82b1c5c3",
        ),
    ] {
        assert_eq!(
            bridge(fixture),
            words(&[
                "agent",
                "report",
                "--state",
                "waiting",
                "--conversation",
                next
            ]),
            "{fixture}"
        );
    }
}

#[test]
fn an_exit_announces_no_change_of_conversation() {
    assert_eq!(bridge("session-end"), None);
}

#[test]
fn a_payload_without_its_conversation_reports_without_one() {
    let payload = r#"{"hook_event_name":"Stop"}"#;

    assert_eq!(
        hooks::bridge_arguments(&hooks::normalize(payload).unwrap()),
        words(&[
            "agent",
            "report",
            "--state",
            "waiting",
            "--outcome",
            "completed"
        ])
    );
}
