//! The wire form of the coordinator kit's commands and replies (#410).
use cockpit_protocol::{
    Command, CoordinatorAuditView, CoordinatorGrantView, InboundMessageView, InboxView, Reply,
    decode_request,
};
use serde_json::json;

#[test]
fn each_coordinator_command_round_trips_with_only_the_fields_it_was_given() {
    let cases = [
        (
            json!({"id": 1, "command": {"kind": "agent-sessions"}}),
            Command::AgentSessions { launch_token: None },
        ),
        (
            json!({"id": 1, "command": {
                "kind": "agent-message", "launch_token": "t", "session": "s1", "text": "rebase"
            }}),
            Command::AgentMessage {
                launch_token: Some("t".to_owned()),
                session: "s1".to_owned(),
                text: "rebase".to_owned(),
            },
        ),
        (
            json!({"id": 1, "command": {"kind": "agent-inbox"}}),
            Command::AgentInbox {
                launch_token: None,
                waiter: None,
            },
        ),
        (
            json!({"id": 1, "command": {"kind": "agent-inbox", "launch_token": "t", "waiter": 7}}),
            Command::AgentInbox {
                launch_token: Some("t".to_owned()),
                waiter: Some(7),
            },
        ),
        (
            json!({"id": 1, "command": {"kind": "agent-start", "dir": "/p/app", "prompt": "fix it"}}),
            Command::AgentStart {
                launch_token: None,
                dir: "/p/app".to_owned(),
                agent: None,
                prompt: Some("fix it".to_owned()),
                clone: None,
            },
        ),
        (
            json!({"id": 1, "command": {"kind": "agent-start", "dir": "/p/app", "agent": "codex"}}),
            Command::AgentStart {
                launch_token: None,
                dir: "/p/app".to_owned(),
                agent: Some("codex".to_owned()),
                prompt: None,
                clone: None,
            },
        ),
        (
            json!({"id": 1, "command": {
                "kind": "session-grant", "session": "s1", "permission": "start-sessions",
                "without_confirmation": true, "per_hour": 3
            }}),
            Command::SessionGrant {
                session: "s1".to_owned(),
                permission: "start-sessions".to_owned(),
                without_confirmation: true,
                per_hour: Some(3),
            },
        ),
        (
            json!({"id": 1, "command": {
                "kind": "session-grant", "session": "s1", "permission": "observe-sessions"
            }}),
            Command::SessionGrant {
                session: "s1".to_owned(),
                permission: "observe-sessions".to_owned(),
                without_confirmation: false,
                per_hour: None,
            },
        ),
        (
            json!({"id": 1, "command": {
                "kind": "session-revoke", "session": "s1", "permission": "message-sessions"
            }}),
            Command::SessionRevoke {
                session: "s1".to_owned(),
                permission: "message-sessions".to_owned(),
            },
        ),
        (
            json!({"id": 1, "command": {"kind": "session-grants", "session": "s1"}}),
            Command::SessionGrants {
                session: "s1".to_owned(),
            },
        ),
    ];
    for (wire, expected) in cases {
        let request = decode_request(&wire.to_string()).unwrap();

        assert_eq!(request.command, expected, "{wire}");
        assert_eq!(serde_json::to_value(request).unwrap(), wire);
    }
}

#[test]
fn coordinator_grants_name_start_terms_only_on_start_sessions() {
    let reply = Reply::CoordinatorGrants {
        session: "s1".to_owned(),
        grants: vec![
            CoordinatorGrantView {
                permission: "observe-sessions".to_owned(),
                grant: 4,
                per_hour: None,
                needs_confirmation: None,
            },
            CoordinatorGrantView {
                permission: "start-sessions".to_owned(),
                grant: 5,
                per_hour: Some(10),
                needs_confirmation: Some(true),
            },
        ],
        audit: vec![CoordinatorAuditView {
            id: 9,
            actor: "s1".to_owned(),
            permission: "message-sessions".to_owned(),
            action: "message".to_owned(),
            target: Some("s2".to_owned()),
            detail: Some("rebase".to_owned()),
            at: 1_700_000_000,
        }],
    };

    let wire = serde_json::to_value(&reply).unwrap();

    assert_eq!(
        wire,
        json!({
            "kind": "coordinator-grants",
            "session": "s1",
            "grants": [
                {"permission": "observe-sessions", "grant": 4},
                {"permission": "start-sessions", "grant": 5, "per_hour": 10,
                 "needs_confirmation": true}
            ],
            "audit": [{
                "id": 9, "actor": "s1", "permission": "message-sessions", "action": "message",
                "target": "s2", "detail": "rebase", "at": 1_700_000_000
            }]
        })
    );
    assert_eq!(serde_json::from_value::<Reply>(wire).unwrap(), reply);
}

#[test]
fn an_inbound_message_names_its_sender_and_delivery() {
    let view = InboundMessageView {
        from: "s1".to_owned(),
        text: "rebase on main".to_owned(),
        delivery: "queued".to_owned(),
        sent_at: 1_700_000_000,
        note: None,
    };

    assert_eq!(
        serde_json::to_value(&view).unwrap(),
        json!({"from": "s1", "text": "rebase on main", "delivery": "queued", "sent_at": 1_700_000_000})
    );
}

/// #703: what one `agent-inbox` found, tagged by `state`.
#[test]
fn an_inbox_reply_names_its_state() {
    let cases = [
        (
            InboxView::Message {
                message: 7,
                line: "[Torlek] Message from session s1".to_owned(),
            },
            json!({"kind": "agent-inbox", "waiter": 3, "inbox": {
                "state": "message", "message": 7, "line": "[Torlek] Message from session s1"
            }}),
        ),
        (
            InboxView::Empty,
            json!({"kind": "agent-inbox", "waiter": 3, "inbox": {"state": "empty"}}),
        ),
        (
            InboxView::Superseded,
            json!({"kind": "agent-inbox", "waiter": 3, "inbox": {"state": "superseded"}}),
        ),
    ];
    for (inbox, wire) in cases {
        let reply = Reply::AgentInbox { waiter: 3, inbox };

        assert_eq!(serde_json::to_value(&reply).unwrap(), wire);
        assert_eq!(serde_json::from_value::<Reply>(wire).unwrap(), reply);
    }
}

/// #703: a message the target's hook collects says so; one typed at its prompt leaves the
/// field out, as before.
#[test]
fn a_queued_message_says_whether_its_hook_collects_it() {
    let collected = Reply::AgentMessageQueued {
        session: "s2".to_owned(),
        message: 7,
        waits: None,
        collected: true,
    };
    let typed = Reply::AgentMessageQueued {
        session: "s2".to_owned(),
        message: 7,
        waits: None,
        collected: false,
    };

    assert_eq!(serde_json::to_value(&collected).unwrap()["collected"], true);
    assert!(
        serde_json::to_value(&typed)
            .unwrap()
            .get("collected")
            .is_none()
    );
}
