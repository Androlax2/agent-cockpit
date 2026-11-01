//! The instruction bundle and launch brief on the wire (#49): topics are read by name, the
//! brief names no session in its request.
use cockpit_protocol::{Command, ContextBudgetView, InstructionTopicView, Reply, decode_request};
use serde_json::json;

#[test]
fn reading_one_topic_round_trips_through_its_wire_form() {
    let wire = json!({"id": 1, "command": {"kind": "instructions-read", "topic": "checks"}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::InstructionsRead {
            topic: Some("checks".to_owned())
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn reading_without_a_topic_asks_for_the_whole_manual() {
    let request = decode_request(r#"{"id": 1, "command": {"kind": "instructions-read"}}"#).unwrap();

    assert_eq!(request.command, Command::InstructionsRead { topic: None });
}

#[test]
fn the_catalog_lists_topics_with_the_manifest_and_version() {
    let reply = Reply::InstructionCatalog {
        version: "0.1.0".to_owned(),
        manifest: "m-0123456789abcdef".to_owned(),
        topics: vec![InstructionTopicView {
            name: "checks".to_owned(),
            summary: "prove a result".to_owned(),
        }],
    };

    assert_eq!(
        serde_json::to_value(&reply).unwrap(),
        json!({
            "kind": "instruction-catalog",
            "version": "0.1.0",
            "manifest": "m-0123456789abcdef",
            "topics": [{"name": "checks", "summary": "prove a result"}]
        })
    );
}

#[test]
fn a_brief_request_carries_only_the_launch_token() {
    let wire = json!({
        "id": 2,
        "command": {"kind": "agent-brief", "launch_token": "abc", "session": "someone-else"}
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::AgentBrief {
            launch_token: Some("abc".to_owned())
        }
    );
}

#[test]
fn a_brief_reply_carries_its_context_budget() {
    let reply = Reply::AgentBrief {
        brief: "# Cockpit launch brief".to_owned(),
        manifest: "m-0123456789abcdef".to_owned(),
        budget: ContextBudgetView {
            brief_bytes: 22,
            catalog_bytes: 280,
            on_demand_bytes: 7478,
        },
    };

    assert_eq!(
        serde_json::to_value(&reply).unwrap(),
        json!({
            "kind": "agent-brief",
            "brief": "# Cockpit launch brief",
            "manifest": "m-0123456789abcdef",
            "budget": {"brief_bytes": 22, "catalog_bytes": 280, "on_demand_bytes": 7478}
        })
    );
}
