//! The instruction bundle and launch brief on the wire (#49): topics are read by name, the
//! brief names no session in its request.
use cockpit_protocol::{
    Command, ContextBudgetView, ContextUsage, InstalledSkillView, InstructionTopicView,
    PackDeliveryView, PackSkillStateView, PackSkillView, Reply, SkillPackView, decode_request,
};
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
        packet: Some("# Task packet".to_owned()),
        manifest: "m-0123456789abcdef".to_owned(),
        budget: Box::new(ContextBudgetView {
            brief_bytes: 22,
            packet_bytes: 13,
            catalog_bytes: 280,
            mcp_instructions_bytes: 95,
            pack_catalog_bytes: 504,
            on_demand_bytes: 7478,
        }),
        harness_context: Some(ContextUsage {
            tokens: 45_123,
            window: None,
        }),
    };

    assert_eq!(
        serde_json::to_value(&reply).unwrap(),
        json!({
            "kind": "agent-brief",
            "brief": "# Cockpit launch brief",
            "packet": "# Task packet",
            "manifest": "m-0123456789abcdef",
            "budget": {
                "brief_bytes": 22,
                "packet_bytes": 13,
                "catalog_bytes": 280,
                "mcp_instructions_bytes": 95,
                "pack_catalog_bytes": 504,
                "on_demand_bytes": 7478
            },
            "harness_context": { "tokens": 45_123 }
        })
    );
}

/// #781: a brief without a task packet, as a daemon before it answers, still reads.
#[test]
fn a_brief_reply_without_a_packet_reads_as_none() {
    let wire = json!({
        "kind": "agent-brief",
        "brief": "# Cockpit launch brief",
        "manifest": "m-0123456789abcdef",
        "budget": {"brief_bytes": 22, "catalog_bytes": 280, "on_demand_bytes": 7478}
    });

    let reply: Reply = serde_json::from_value(wire).unwrap();

    let Reply::AgentBrief { packet, budget, .. } = reply else {
        panic!("{reply:?}");
    };
    assert_eq!((packet, budget.packet_bytes), (None, 0));
}

/// #786: a daemon from before the skill pack counts none.
#[test]
fn a_brief_budget_without_the_skill_pack_reads_as_zero() {
    let wire = json!({"brief_bytes": 22, "catalog_bytes": 280, "on_demand_bytes": 7478});

    let budget: ContextBudgetView = serde_json::from_value(wire).unwrap();

    assert_eq!(budget.pack_catalog_bytes, 0);
}

#[test]
fn the_skill_pack_listing_is_asked_with_no_field() {
    let wire = json!({"id": 3, "command": {"kind": "skill-pack"}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(request.command, Command::SkillPack);
}

#[test]
fn the_skill_pack_listing_names_the_pin_each_skill_and_the_installed_ones() {
    let reply = Reply::SkillPack {
        pack: Box::new(SkillPackView {
            source: "https://github.com/mattpocock/skills".to_owned(),
            commit: "4ae2317".to_owned(),
            tree: "a73b182".to_owned(),
            licence: "MIT".to_owned(),
            enabled: true,
            skills: vec![PackSkillView {
                name: "tdd".to_owned(),
                deliveries: vec![
                    PackDeliveryView {
                        harness: "claude".to_owned(),
                        state: PackSkillStateView::Shadowed {
                            by: "/home/o/.claude/skills/tdd".to_owned(),
                        },
                    },
                    PackDeliveryView {
                        harness: "opencode".to_owned(),
                        state: PackSkillStateView::Delivered,
                    },
                ],
                is_starter: true,
                is_user_invoked: false,
                grants: Vec::new(),
            }],
            installed: vec![InstalledSkillView {
                name: "tdd".to_owned(),
                scope: "user".to_owned(),
                path: "/home/o/.claude/skills/tdd".to_owned(),
                collides: true,
            }],
        }),
    };

    assert_eq!(
        serde_json::to_value(&reply).unwrap(),
        json!({
            "kind": "skill-pack",
            "pack": {
                "source": "https://github.com/mattpocock/skills",
                "commit": "4ae2317",
                "tree": "a73b182",
                "licence": "MIT",
                "enabled": true,
                "skills": [{
                    "name": "tdd",
                    "deliveries": [
                        {"harness": "claude", "state": {"shadowed": {"by": "/home/o/.claude/skills/tdd"}}},
                        {"harness": "opencode", "state": "delivered"}
                    ],
                    "is_starter": true,
                    "is_user_invoked": false,
                    "grants": []
                }],
                "installed": [{
                    "name": "tdd",
                    "scope": "user",
                    "path": "/home/o/.claude/skills/tdd",
                    "collides": true
                }]
            }
        })
    );
}
