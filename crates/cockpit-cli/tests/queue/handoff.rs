//! Process-level handoff command: the caller supplies the snapshot, including on refusal.
use super::*;

#[tokio::test]
async fn handoff_sends_the_explicit_snapshot_and_prints_the_task() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Task { task: Box::new(serde_json::from_value(serde_json::json!({
            "id": 7, "project": "app", "title": "Selected task", "notes": "", "kind": "task", "priority": 2,
            "status": "doing", "is_blocker_report": false, "doing_session": "s0123456789ab",
            "is_queued": false, "waits": [], "images": [], "intent_revision": 2, "state_revision": 4,
            "created_at": 1, "changed_at": 2, "next": {"kind": "in-progress", "session":"s0123456789ab"}
        })).unwrap()) }),
    );
    let output = home
        .cockpit(&[
            "todo",
            "handoff",
            "7",
            "s0123456789ab",
            "--revision",
            "2",
            "--state-revision",
            "3",
            "--launch",
            "4",
        ])
        .await;
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::TaskHandoff {
            item: 7,
            intent_revision: 2,
            state_revision: 3,
            session: "s0123456789ab".into(),
            expected_launch: 4
        }
    );
    assert!(text(&output.stdout).contains("s0123456789ab"));
}

#[tokio::test]
async fn handoff_requires_an_explicit_snapshot_before_connecting() {
    let home = Home::new();
    let output = home
        .cockpit(&["todo", "handoff", "7", "s0123456789ab"])
        .await;
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("--revision"));
}

#[tokio::test]
async fn a_stale_handoff_exits_with_refusal_without_refreshing_the_identity() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Error(ProtocolError::new(
            ErrorCode::StaleRevision,
            "the selected task changed",
        )),
    );
    let output = home
        .cockpit(&[
            "todo",
            "handoff",
            "7",
            "s0123456789ab",
            "--revision",
            "2",
            "--state-revision",
            "3",
            "--launch",
            "4",
        ])
        .await;
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("the selected task changed"));
    assert!(matches!(
        daemon.await.unwrap(),
        Command::TaskHandoff {
            intent_revision: 2,
            state_revision: 3,
            expected_launch: 4,
            ..
        }
    ));
}

#[tokio::test]
async fn handoff_targets_exposes_the_launch_number_to_select_explicitly() {
    let home = Home::new();
    let session = serde_json::from_value(serde_json::json!({
        "id":"s0123456789ab", "agent":"fake", "checkout":1, "holds_checkout":false,
        "working_dir":"/p/app", "state":{"kind":"running"}, "revision":1, "created_at":1,"changed_at":1,
        "launch":{"number":4,"kind":"start","launched_at":1}
    })).unwrap();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Sessions {
            sessions: vec![session],
        }),
    );
    let output = home.cockpit(&["todo", "targets"]).await;
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(daemon.await.unwrap(), Command::SessionList { ended: false });
    assert!(
        text(&output.stdout).contains("s0123456789ab\t4\trunning\t/p/app"),
        "{}",
        text(&output.stdout)
    );
}
