//! The wire form of the queue commands and views (#43).
use cockpit_protocol::{
    Command, DemandView, QueueExplanationView, QueueSessionNoteView, QueueStepView, Reply,
    decode_request,
};
use serde_json::json;

#[test]
fn queue_explain_without_an_item_asks_for_every_explanation() {
    let wire = json!({"id": 1, "command": {"kind": "queue-explain"}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(request.command, Command::QueueExplain { item: None });
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn queue_explain_names_one_item() {
    let wire = json!({"id": 2, "command": {"kind": "queue-explain", "item": 7}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(request.command, Command::QueueExplain { item: Some(7) });
}

#[test]
fn queue_dispatch_carries_no_arguments() {
    let wire = json!({"id": 3, "command": {"kind": "queue-dispatch"}});

    assert_eq!(
        decode_request(&wire.to_string()).unwrap().command,
        Command::QueueDispatch
    );
}

#[test]
fn an_explanation_of_a_waiting_step_keeps_its_sessions_and_leaves_out_what_is_absent() {
    let explanation = QueueExplanationView {
        item: 4,
        project: "web".to_owned(),
        title: "fix login".to_owned(),
        step: Some(QueueStepView {
            run: 2,
            position: 1,
            name: "implement".to_owned(),
        }),
        reason: "no-session-free".to_owned(),
        why: "none of the 1 sessions of project web can take it now".to_owned(),
        limited_by: None,
        sessions: vec![QueueSessionNoteView {
            session: "s0123456789ab".to_owned(),
            why: "its agent is working".to_owned(),
        }],
        behind: None,
        action: "wait: a later pass places it once a session is free".to_owned(),
        action_by: "cockpit".to_owned(),
        priority: 2,
        aging_boost: 1,
        demand: DemandView {
            cpu_millis: 750,
            memory_mib: 1536,
        },
        headroom_percent: 50,
        waiting_since: Some(1_800_000_000),
        evaluated_at: 1_800_000_900,
        next_evaluation_at: 1_800_000_930,
    };

    let wire = serde_json::to_value(Reply::QueueExplanations {
        explanations: vec![explanation.clone()],
    })
    .unwrap();

    assert_eq!(
        wire,
        json!({
            "kind": "queue-explanations",
            "explanations": [{
                "item": 4,
                "project": "web",
                "title": "fix login",
                "step": {"run": 2, "position": 1, "name": "implement"},
                "reason": "no-session-free",
                "why": "none of the 1 sessions of project web can take it now",
                "sessions": [{"session": "s0123456789ab", "why": "its agent is working"}],
                "action": "wait: a later pass places it once a session is free",
                "action_by": "cockpit",
                "priority": 2,
                "aging_boost": 1,
                "demand": {"cpu_millis": 750, "memory_mib": 1536},
                "headroom_percent": 50,
                "waiting_since": 1_800_000_000,
                "evaluated_at": 1_800_000_900,
                "next_evaluation_at": 1_800_000_930
            }]
        })
    );
    let read: Reply = serde_json::from_value(wire).unwrap();
    assert_eq!(
        read,
        Reply::QueueExplanations {
            explanations: vec![explanation]
        }
    );
}
