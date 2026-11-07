//! A context compaction on the agent and owner wires (#782): additive fields that older peers
//! neither send nor need.
use cockpit_protocol::{
    AgentReport, CompactionPhase, CompactionReport, CompactionStatus, CompactionTrigger,
    CompactionView, SessionReport,
};
use serde_json::json;

#[test]
fn a_compaction_report_names_its_phase_trigger_and_sizes_in_kebab_case() {
    let report = AgentReport {
        compaction: Some(CompactionReport {
            phase: CompactionPhase::Ended,
            trigger: Some(CompactionTrigger::Manual),
            tokens_before: Some(268_364),
            tokens_after: Some(30_470),
        }),
        ..AgentReport::default()
    };

    let wire = serde_json::to_value(&report).unwrap();

    assert_eq!(wire["compaction"]["phase"], "ended");
    assert_eq!(wire["compaction"]["trigger"], "manual");
    assert_eq!(wire["compaction"]["tokens_before"], 268_364);
    assert_eq!(serde_json::from_value::<AgentReport>(wire).unwrap(), report);
}

#[test]
fn a_report_from_an_older_agent_client_reads_without_a_compaction() {
    let report: AgentReport = serde_json::from_value(json!({ "state": "working" })).unwrap();

    assert_eq!(report.compaction, None);
}

#[test]
fn a_report_without_a_compaction_writes_none_for_an_older_daemon() {
    let wire = serde_json::to_value(AgentReport::default()).unwrap();

    assert!(wire.get("compaction").is_none(), "{wire}");
}

#[test]
fn a_session_report_from_an_older_daemon_reads_without_a_compaction() {
    let wire = json!({
        "session": "s1", "state": "waiting", "label": null, "doing": null,
        "needs_input": null, "question_source": null, "reported_at": 42
    });

    let report: SessionReport = serde_json::from_value(wire).unwrap();

    assert_eq!(report.compaction, None);
}

#[test]
fn a_session_report_carries_the_latest_compaction_with_its_status() {
    let wire = json!({
        "session": "s1", "state": "waiting", "label": null, "doing": null,
        "needs_input": null, "question_source": null, "reported_at": 42,
        "compaction": { "status": "failed", "at": 40 }
    });

    let report: SessionReport = serde_json::from_value(wire).unwrap();

    assert_eq!(
        report.compaction,
        Some(CompactionView {
            status: CompactionStatus::Failed,
            trigger: None,
            tokens_before: None,
            tokens_after: None,
            seconds: None,
            at: 40,
        })
    );
}

#[test]
fn an_unknown_compaction_phase_is_refused() {
    let refused = serde_json::from_value::<AgentReport>(json!({
        "compaction": { "phase": "paused" }
    }));

    assert!(refused.is_err());
}
