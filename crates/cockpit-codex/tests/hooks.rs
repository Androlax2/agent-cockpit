//! Historical 0.161.0 hook payloads and their reports. The subscribed input schemas
//! also match upstream 0.166.0; see fixtures/codex-0.166.0/README.md.

mod common;

use cockpit_codex::hooks::{self, HOOK_EVENTS};
use cockpit_provider_api::{
    CompactionMoment, CompactionPhase, CompactionTrigger, ConversationId, EventKind, TurnOutcome,
};

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
    assert_eq!(checked, 13);
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

/// Codex sends `Stop` only for a completed turn; a failed one's outcome comes from the
/// session launcher (`tests/session.rs`).
#[test]
fn a_stop_is_a_completed_turn_reported_as_waiting_with_its_outcome() {
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
            "--outcome",
            "completed",
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

/// The launch's report ledger adds the old thread's end before this report (#408).
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

/// A compaction can run in the middle of a turn: its `SessionStart` proves no wait. It is a
/// known event of the compaction, not an unknown one (#782).
#[test]
fn a_session_start_after_a_compaction_is_a_known_event_that_reports_nothing() {
    let event = hooks::normalize(&common::fixture("session-start-compact")).unwrap();

    assert_eq!(event.kind, EventKind::ContextCompacted);
    assert_eq!(event.observation(), None);
    assert_eq!(hooks::bridge_arguments(&event), None);
    assert!(hooks::starts_context(&event), "the brief must still return");
}

#[test]
fn a_pre_compact_and_a_post_compact_are_the_two_moments_of_a_compaction() {
    assert_eq!(
        kind("pre-compact"),
        EventKind::Compaction(CompactionMoment {
            phase: CompactionPhase::Started,
            trigger: Some(CompactionTrigger::Auto),
            tokens_before: None,
            tokens_after: None,
        })
    );
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
    assert_eq!(
        report("pre-compact"),
        reported(&[
            "agent",
            "report",
            "--compaction",
            "started",
            "--compaction-trigger",
            "auto",
            "--conversation",
            THREAD
        ])
    );
}

/// The rollout holds the sizes: at `PreCompact` its last `token_count`, at `PostCompact` the
/// first one after the `compacted` record.
#[test]
fn a_compaction_reports_the_context_sizes_the_rollout_holds() {
    let directory = tempfile::tempdir().unwrap();
    let rollout = directory.path().join("rollout.jsonl");
    let records =
        std::fs::read_to_string(common::fixtures_dir().join("rollout-compaction.jsonl")).unwrap();
    let measured = |fixture: &str, records: &str| {
        std::fs::write(&rollout, records).unwrap();
        let mut payload: serde_json::Value =
            serde_json::from_str(&common::fixture(fixture)).unwrap();
        payload["transcript_path"] = rollout.to_str().unwrap().into();
        hooks::measured_arguments(&hooks::normalize(&payload.to_string()).unwrap()).unwrap()
    };
    let until_compaction: String = records
        .lines()
        .take(3)
        .map(|line| format!("{line}\n"))
        .collect();

    let before = measured("pre-compact", &until_compaction);
    let after = measured("post-compact", &records);
    let after_without_a_model_call_yet = measured(
        "post-compact",
        &records
            .lines()
            .take(4)
            .map(|line| format!("{line}\n"))
            .collect::<String>(),
    );

    assert!(
        before
            .windows(2)
            .any(|pair| pair == ["--compaction-tokens-before", "243752"]),
        "{before:?}"
    );
    assert!(
        after
            .windows(2)
            .any(|pair| pair == ["--compaction-tokens-after", "30525"]),
        "{after:?}"
    );
    assert!(
        !after_without_a_model_call_yet
            .iter()
            .any(|argument| argument == "--compaction-tokens-after"),
        "{after_without_a_model_call_yet:?}"
    );
}

#[test]
fn a_compaction_whose_rollout_cannot_be_read_reports_without_sizes() {
    let event = hooks::normalize(&common::fixture("pre-compact")).unwrap();

    assert_eq!(
        hooks::measured_arguments(&event),
        hooks::bridge_arguments(&event)
    );
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
    let payload = r#"{"hook_event_name":"PreToolUse","session_id":"t"}"#;

    let event = hooks::normalize(payload).unwrap();

    assert_eq!(event.kind, EventKind::Unknown);
    assert_eq!(event.name, "PreToolUse");
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
        reported(&[
            "agent",
            "report",
            "--state",
            "waiting",
            "--outcome",
            "completed"
        ])
    );
}
