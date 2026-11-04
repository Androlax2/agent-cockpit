//! Owner-selected placement uses the real daemon/socket, admission and delivery path.
use super::*;
use cockpit_protocol::TaskView;

async fn item(client: &Connection, id: i64) -> TaskView {
    match request(
        client,
        Command::TaskShow {
            launch_token: None,
            item: id,
        },
    )
    .await
    .unwrap()
    {
        Reply::Task { task } => *task,
        other => panic!("task-show: {other:?}"),
    }
}

async fn handoff_command(client: &Connection, task: &TaskView, session: &str) -> Command {
    let Reply::Sessions { sessions } = request(client, Command::SessionList { ended: false })
        .await
        .unwrap()
    else {
        panic!("session-list")
    };
    let selected = sessions.iter().find(|s| s.id == session).unwrap();
    serde_json::from_value(serde_json::json!({
        "kind": "task-handoff", "item": task.id,
        "intent_revision": task.intent_revision, "state_revision": task.state_revision,
        "session": session, "expected_launch": selected.launch.as_ref().unwrap().number
    }))
    .expect("the owner handoff command must exist")
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_handoff_places_only_the_selected_item_and_session_and_delivers_once() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let other = home
        .session_reporting(&client, &root, AgentState::Waiting)
        .await;
    let selected = home
        .session_reporting(&client, &root, AgentState::Waiting)
        .await;
    let competing = queued_item(&client, "A competing queued task").await;
    let id = queued_item(&client, "Selected task").await;
    request(
        &client,
        Command::TaskQueue {
            item: id,
            queued: false,
        },
    )
    .await
    .unwrap();
    let before = item(&client, id).await;
    let command = handoff_command(&client, &before, &selected.id).await;
    let reply = request(&client, command.clone()).await.unwrap();
    assert!(
        matches!(reply, Reply::Task { ref task } if task.id == id && task.doing_session.as_deref() == Some(selected.id.as_str()))
    );
    let after = item(&client, id).await;
    assert_eq!(after.doing_session.as_deref(), Some(selected.id.as_str()));
    assert_eq!(after.status, TaskStatusView::Doing);
    assert_eq!(item(&client, competing).await.status, TaskStatusView::Todo);
    let messages = home.messages_of(&selected.id, 1).await;
    assert_eq!(messages.len(), 1);
    assert!(messages[0].contains(&id.to_string()));
    let error = request(&client, command).await.unwrap_err();
    assert_eq!(refused(Err(error)).code, ErrorCode::StaleRevision);
    let database = home.database();
    let read = database.read().await.unwrap();
    let placements = cockpit_scheduling::placements(&read).await.unwrap();
    assert_eq!(placements.len(), 1);
    assert!(placements[0].reservation.is_some());
    assert_ne!(placements[0].session.as_str(), other.id);
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_intent_state_and_session_launch_refuse_without_placement() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let selected = home
        .session_reporting(&client, &root, AgentState::Waiting)
        .await;
    let id = queued_item(&client, "Snapshot task").await;
    let before = item(&client, id).await;
    let stale_intent = handoff_command(&client, &before, &selected.id).await;
    request(
        &client,
        Command::TaskEdit {
            launch_token: None,
            item: id,
            intent_revision: Some(before.intent_revision),
            title: Some("Changed".into()),
            notes: None,
            item_kind: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        refused(request(&client, stale_intent).await).code,
        ErrorCode::StaleRevision
    );
    let stale_state = handoff_command(&client, &item(&client, id).await, &selected.id).await;
    request(
        &client,
        Command::TaskQueue {
            item: id,
            queued: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        refused(request(&client, stale_state).await).code,
        ErrorCode::StaleRevision
    );
    let stale_launch = handoff_command(&client, &item(&client, id).await, &selected.id).await;
    request(
        &client,
        Command::SessionRestart {
            session: selected.id.clone(),
        },
    )
    .await
    .unwrap();
    let deadline = Instant::now() + GENEROUS;
    loop {
        let current = handoff_command(&client, &item(&client, id).await, &selected.id).await;
        if current != stale_launch {
            break;
        }
        assert!(Instant::now() < deadline, "restart did not replace launch");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        refused(request(&client, stale_launch).await).code,
        ErrorCode::StaleRevision
    );
    assert_eq!(item(&client, id).await.status, TaskStatusView::Todo);
    let database = home.database();
    let read = database.read().await.unwrap();
    assert!(
        cockpit_scheduling::placements(&read)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_busy_and_paused_handoffs_leave_task_and_reservations_unchanged() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let busy = home
        .session_reporting(&client, &root, AgentState::Working)
        .await;
    let idle = home
        .session_reporting(&client, &root, AgentState::Waiting)
        .await;
    let id = queued_item(&client, "No bypass").await;
    request(
        &client,
        Command::TaskQueue {
            item: id,
            queued: false,
        },
    )
    .await
    .unwrap();
    let before = item(&client, id).await;
    let command = handoff_command(&client, &before, &idle.id).await;
    let agent = Connection::open(daemon.agent_socket_path(), GENEROUS)
        .await
        .unwrap();
    assert_eq!(
        refused(request(&agent, command.clone()).await).code,
        ErrorCode::NotAuthorized
    );
    let busy_command = handoff_command(&client, &before, &busy.id).await;
    let error = refused(request(&client, busy_command).await);
    assert_eq!(error.code, ErrorCode::Refused);
    assert!(error.message.contains("working"), "{error:?}");
    request(&client, Command::ResourceQueue { paused: true })
        .await
        .unwrap();
    let error = refused(request(&client, command).await);
    assert_eq!(error.code, ErrorCode::Refused);
    assert!(error.message.contains("paused"), "{error:?}");
    assert_eq!(item(&client, id).await, before);
    let database = home.database();
    let read = database.read().await.unwrap();
    assert!(
        cockpit_scheduling::placements(&read)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        cockpit_resource_admission::in_use(
            &read,
            i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            )
            .unwrap()
        )
        .await
        .unwrap()
        .all,
        cockpit_resource_admission::Demand::default()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn handoff_refuses_cross_project_and_running_workflow_items() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let other_root = home.path().join("projects/other");
    std::fs::create_dir_all(&other_root).unwrap();
    git(&other_root, &["init", "--quiet", "--initial-branch=main"]);
    git(
        &other_root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=f@example.com",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "initial",
        ],
    );
    request(
        &client,
        Command::ProjectAdd {
            path: other_root.display().to_string(),
            name: None,
        },
    )
    .await
    .unwrap();
    let wrong = home
        .session_reporting(&client, &other_root, AgentState::Waiting)
        .await;
    let right = home
        .session_reporting(&client, &root, AgentState::Waiting)
        .await;
    let id = queued_item(&client, "Project-bound task").await;
    let command = handoff_command(&client, &item(&client, id).await, &wrong.id).await;
    assert_eq!(
        refused(request(&client, command).await).code,
        ErrorCode::Refused
    );
    request(
        &client,
        Command::WorkflowStart {
            item: id,
            preset: Some("code-change".into()),
        },
    )
    .await
    .unwrap();
    let command = handoff_command(&client, &item(&client, id).await, &right.id).await;
    let error = refused(request(&client, command).await);
    assert_eq!(error.code, ErrorCode::Refused);
    assert!(error.message.contains("workflow"));
    let database = home.database();
    let read = database.read().await.unwrap();
    assert!(
        cockpit_scheduling::placements(&read)
            .await
            .unwrap()
            .is_empty()
    );
}
