//! The wire form of the session commands and views (#31).
use cockpit_protocol::{
    Command, LaunchKindView, LaunchView, Reply, SessionStateView, SessionView, StopTargetView,
    decode_request,
};
use serde_json::json;

#[test]
fn a_session_new_round_trips_with_only_the_options_it_was_given() {
    let wire = json!({
        "id": 1,
        "command": {"kind": "session-new", "dir": "/p/app", "clone": "feature/x"}
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::SessionNew {
            dir: "/p/app".to_owned(),
            agent: None,
            prompt: None,
            clone: Some("feature/x".to_owned()),
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn a_session_list_without_ended_lists_the_open_sessions() {
    let wire = json!({"id": 2, "command": {"kind": "session-list"}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(request.command, Command::SessionList { ended: false });
}

#[test]
fn a_session_view_names_its_state_and_latest_launch_on_the_wire() {
    let view = SessionView {
        id: "s0123456789ab".to_owned(),
        agent: "claude".to_owned(),
        checkout: 4,
        holds_checkout: true,
        working_dir: "/p/.parallel/app/feature-x".to_owned(),
        conversation: Some("c-1".to_owned()),
        design_link: None,
        state: SessionStateView::Stopping {
            to: StopTargetView::Restart,
        },
        reason: None,
        revision: 3,
        created_at: 100,
        changed_at: 200,
        launch: Some(LaunchView {
            number: 9,
            kind: LaunchKindView::Restore,
            launched_at: 150,
        }),
        changed_start_files: Vec::new(),
        restart_pending: false,
        finished: false,
    };

    let wire = serde_json::to_value(Reply::Session {
        session: Box::new(view),
    })
    .unwrap();

    assert_eq!(
        wire,
        json!({
            "kind": "session",
            "session": {
                "id": "s0123456789ab",
                "agent": "claude",
                "checkout": 4,
                "holds_checkout": true,
                "working_dir": "/p/.parallel/app/feature-x",
                "conversation": "c-1",
                "design_link": null,
                "state": {"kind": "stopping", "to": "restart"},
                "reason": null,
                "revision": 3,
                "created_at": 100,
                "changed_at": 200,
                "launch": {"number": 9, "kind": "restore", "launched_at": 150}
            }
        })
    );
}

#[test]
fn a_session_state_or_launch_kind_from_a_newer_daemon_reads_as_unknown() {
    let state: SessionStateView = serde_json::from_value(json!({"kind": "hibernating"})).unwrap();
    let kind: LaunchKindView = serde_json::from_value(json!("migrate")).unwrap();

    assert_eq!(
        (state, kind),
        (SessionStateView::Unknown, LaunchKindView::Unknown)
    );
}
