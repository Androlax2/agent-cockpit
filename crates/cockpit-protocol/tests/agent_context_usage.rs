//! The harness's context usage on the agent wire and in the brief's reply (#479): additive
//! fields that older peers neither send nor need.
use cockpit_protocol::{AgentReport, ContextBudgetView, ContextUsage, Reply, TurnOutcome};
use serde_json::json;

#[test]
fn a_turns_end_report_carries_the_harness_context_tokens_and_window() {
    let report = AgentReport {
        outcome: Some(TurnOutcome::Completed),
        context: Some(ContextUsage {
            tokens: 45_123,
            window: Some(258_400),
        }),
        ..AgentReport::default()
    };

    let wire = serde_json::to_value(&report).unwrap();

    assert_eq!(
        wire["context"],
        json!({ "tokens": 45_123, "window": 258_400 })
    );
    assert_eq!(serde_json::from_value::<AgentReport>(wire).unwrap(), report);
}

#[test]
fn a_context_without_a_window_writes_none() {
    let wire = serde_json::to_value(ContextUsage {
        tokens: 45_123,
        window: None,
    })
    .unwrap();

    assert_eq!(wire, json!({ "tokens": 45_123 }));
}

#[test]
fn a_report_from_an_older_agent_client_reads_without_a_context() {
    let report: AgentReport = serde_json::from_value(json!({ "state": "waiting" })).unwrap();

    assert_eq!(report.context, None);
}

#[test]
fn a_report_without_a_context_writes_none_for_an_older_daemon() {
    let wire = serde_json::to_value(AgentReport::default()).unwrap();

    assert!(wire.get("context").is_none(), "{wire}");
}

fn budget() -> ContextBudgetView {
    ContextBudgetView {
        brief_bytes: 1,
        packet_bytes: 0,
        catalog_bytes: 0,
        mcp_instructions_bytes: 0,
        pack_catalog_bytes: 0,
        on_demand_bytes: 2,
    }
}

#[test]
fn a_brief_from_an_older_daemon_reads_without_a_harness_context() {
    let wire = json!({
        "kind": "agent-brief", "brief": "b", "manifest": "m-1",
        "budget": serde_json::to_value(budget()).unwrap()
    });

    let reply: Reply = serde_json::from_value(wire).unwrap();

    assert!(
        matches!(
            reply,
            Reply::AgentBrief {
                harness_context: None,
                ..
            }
        ),
        "{reply:?}"
    );
}
