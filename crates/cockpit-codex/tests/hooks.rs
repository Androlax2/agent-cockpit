//! Conformance with the hook payloads pinned for `CONFORMANCE_VERSION`, and the reports they
//! become.

mod common;

use cockpit_codex::hooks::{self, HOOK_EVENTS};
use cockpit_provider_api::{ConversationId, EventKind, TurnOutcome};

const THREAD: &str = "01a19800-e8f3-70e3-b788-d9601b03debb";

fn kind(fixture: &str) -> EventKind {
    hooks::normalize(&common::fixture(fixture)).unwrap().kind
}

fn report(fixture: &str) -> Option<Vec<String>> {
    hooks::bridge_arguments(&hooks::normalize(&common::fixture(fixture)).unwrap())
}

fn reported(words: &[&str]) -> Option<Vec<String>> {
    Some(words.iter().map(|word| (*word).to_owned()).collect())
}

/// Each pinned payload carries every field its pinned input schema requires, and nothing the
/// schema does not know: a payload that drifted from 0.161.0 shows here.
#[test]
fn every_pinned_payload_matches_its_pinned_input_schema() {
    let schemas = common::fixtures_dir().join("schemas");
    let fixtures = std::fs::read_dir(common::fixtures_dir()).unwrap();
    let mut checked = 0;
    for entry in fixtures {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let payload: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let event = payload["hook_event_name"].as_str().unwrap();
        let schema_name = HOOK_EVENTS
            .iter()
            .find(|subscribed| subscribed.name == event)
            .map(|subscribed| subscribed.label.replace('_', "-"))
            .unwrap_or_else(|| panic!("{} is not a subscribed event", path.display()));
        let schema: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(schemas.join(format!("{schema_name}.json"))).unwrap(),
        )
        .unwrap();
        let fields = payload.as_object().unwrap();
        for required in schema["required"].as_array().unwrap() {
            let required = required.as_str().unwrap();
            assert!(
                fields.contains_key(required),
                "{}: {required}",
                path.display()
            );
        }
        for field in fields.keys() {
            assert!(
                schema["properties"].get(field).is_some(),
                "{}: {field} is not in the schema",
                path.display()
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 10);
}

#[test]
fn every_subscribed_event_has_a_pinned_input_schema() {
    for event in HOOK_EVENTS {
        let schema = common::fixtures_dir()
            .join("schemas")
            .join(format!("{}.json", event.label.replace('_', "-")));
        let schema: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&schema).unwrap()).unwrap();

        assert_eq!(
            schema["properties"]["hook_event_name"]["const"], event.name,
            "{event:?}"
        );
    }
}

#[test]
fn a_session_start_names_its_thread() {
    assert_eq!(
        kind("session-start"),
        EventKind::Started {
            conversation: Some(ConversationId::new(THREAD).unwrap())
        }
    );
}

#[test]
fn a_prompt_is_working_and_clears_the_agents_question() {
    assert_eq!(
        report("user-prompt-submit"),
        reported(&[
            "agent",
            "report",
            "--state",
            "working",
            "--prompt",
            "--conversation",
            THREAD
        ])
    );
}

#[test]
fn a_tool_use_is_working_with_the_tool_kept() {
    assert_eq!(
        kind("post-tool-use"),
        EventKind::ToolUsed {
            tool: "Bash".to_owned()
        }
    );
    assert_eq!(
        report("post-tool-use"),
        reported(&[
            "agent",
            "report",
            "--state",
            "working",
            "--conversation",
            THREAD
        ])
    );
}

/// The tool input is the agent's text; the owner reads the request in the terminal.
#[test]
fn a_permission_request_asks_a_plain_question_never_the_tool_input() {
    assert_eq!(
        report("permission-request"),
        reported(&[
            "agent",
            "report",
            "--state",
            "waiting",
            "--dialog",
            "Codex is waiting for you in its terminal",
            "--conversation",
            THREAD
        ])
    );
}

/// No `--outcome`: the adapter does not report `outcome`, since a failed turn sends no hook.
#[test]
fn a_stop_is_a_completed_turn_reported_as_waiting_without_an_outcome() {
    assert_eq!(
        kind("stop"),
        EventKind::TurnEnded {
            outcome: TurnOutcome::Completed
        }
    );
    assert_eq!(
        report("stop"),
        reported(&[
            "agent",
            "report",
            "--state",
            "waiting",
            "--conversation",
            THREAD
        ])
    );
}

#[test]
fn an_interrupt_leaves_codex_waiting_at_its_prompt() {
    assert_eq!(kind("interrupt"), EventKind::Idle);
    assert_eq!(
        report("interrupt"),
        reported(&[
            "agent",
            "report",
            "--state",
            "waiting",
            "--conversation",
            "01a19807-15b6-70f2-aa31-ea48e845bb46"
        ])
    );
}

#[test]
fn a_resumed_session_start_names_the_resumed_thread() {
    assert_eq!(
        report("session-start-resume"),
        reported(&[
            "agent",
            "report",
            "--state",
            "waiting",
            "--conversation",
            "01a19800-5720-7903-9fa3-a2f2ebc5386a"
        ])
    );
}

/// Known gap (#408): nothing announces the old thread's end, so the daemon refuses this.
#[test]
fn a_session_start_after_clear_names_the_new_thread() {
    assert_eq!(
        report("session-start-clear"),
        reported(&[
            "agent",
            "report",
            "--state",
            "waiting",
            "--conversation",
            "01a19807-3643-77e2-a2ad-bf0c4da61e26"
        ])
    );
}

#[test]
fn a_subagents_event_reports_nothing() {
    assert_eq!(kind("post-tool-use-subagent"), EventKind::Unknown);
    assert_eq!(report("post-tool-use-subagent"), None);
}

/// The owner must answer it, whichever agent asked.
#[test]
fn a_subagents_permission_request_still_needs_you() {
    assert_eq!(
        kind("permission-request-subagent"),
        EventKind::NeedsYou { question: None }
    );
}

#[test]
fn a_session_start_after_a_compaction_reports_nothing() {
    let payload = common::fixture("session-start").replace("\"startup\"", "\"compact\"");

    let event = hooks::normalize(&payload).unwrap();

    assert_eq!(event.kind, EventKind::Unknown);
    assert_eq!(hooks::bridge_arguments(&event), None);
}

#[test]
fn a_session_end_reports_nothing() {
    let event = hooks::normalize(
        r#"{"session_id":"t","transcript_path":null,"cwd":"/work","hook_event_name":"SessionEnd","reason":"other"}"#,
    )
    .unwrap();

    assert_eq!(event.kind, EventKind::Ended);
    assert_eq!(hooks::bridge_arguments(&event), None);
}

#[test]
fn an_unknown_event_is_kept_raw() {
    let payload = r#"{"hook_event_name":"PreCompact","session_id":"t"}"#;

    let event = hooks::normalize(payload).unwrap();

    assert_eq!(event.kind, EventKind::Unknown);
    assert_eq!(event.name, "PreCompact");
    assert_eq!(event.raw, payload);
}

#[test]
fn a_payload_that_is_not_json_is_refused() {
    assert!(hooks::normalize("not json").is_err());
}

#[test]
fn a_payload_without_an_event_name_is_refused() {
    let refused = hooks::normalize(r#"{"session_id":"t"}"#).unwrap_err();

    assert!(refused.reason.contains("hook_event_name"), "{refused}");
}

#[test]
fn a_payload_naming_no_valid_thread_reports_without_one() {
    let payload = common::fixture("stop").replace(THREAD, "has spaces");

    assert_eq!(
        hooks::bridge_arguments(&hooks::normalize(&payload).unwrap()),
        reported(&["agent", "report", "--state", "waiting"])
    );
}
