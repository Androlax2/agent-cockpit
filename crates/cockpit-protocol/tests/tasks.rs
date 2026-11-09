//! The wire form of the task commands and views (#38).
use cockpit_protocol::{
    Command, ErrorCode, HexBytes, Reply, TaskBudgetSpentView, TaskBudgetView, TaskKindView,
    TaskNextView, TaskStatusView, TaskView, decode_request,
};
use serde_json::json;

#[test]
fn a_task_add_round_trips_with_only_the_fields_it_was_given() {
    let wire = json!({
        "id": 1,
        "command": {
            "kind": "task-add",
            "launch_token": "token",
            "title": "Fix the login",
            "item_kind": "bug",
            "after": [3, 4]
        }
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::TaskAdd {
            launch_token: Some("token".to_owned()),
            project: None,
            title: "Fix the login".to_owned(),
            notes: String::new(),
            item_kind: Some(TaskKindView::Bug),
            priority: None,
            after: vec![3, 4],
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn image_bytes_travel_as_lower_case_hex() {
    let wire = json!({
        "id": 2,
        "command": {"kind": "task-image-add", "item": 7, "data": "89504e47ff00"}
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::TaskImageAdd {
            item: 7,
            data: HexBytes(vec![0x89, 0x50, 0x4e, 0x47, 0xff, 0x00]),
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn a_spent_budget_names_its_item_and_the_minutes_it_allowed() {
    let spent = TaskBudgetSpentView {
        item: 7,
        project: "app".to_owned(),
        title: "Fix the login".to_owned(),
        budget_minutes: 60,
    };

    let wire = serde_json::to_value(&spent).unwrap();

    assert_eq!(
        wire,
        json!({"item": 7, "project": "app", "title": "Fix the login", "budget_minutes": 60})
    );
    assert_eq!(
        serde_json::from_value::<TaskBudgetSpentView>(wire).unwrap(),
        spent
    );
}

#[test]
fn a_task_budget_names_its_minutes_or_no_limit() {
    for (budget, expected) in [
        (json!({"minutes": 90}), TaskBudgetView::Minutes(90)),
        (json!("no-limit"), TaskBudgetView::NoLimit),
    ] {
        let wire = json!({
            "id": 4,
            "command": {"kind": "task-budget", "item": 7, "budget": budget}
        });

        let request = decode_request(&wire.to_string()).unwrap();

        assert_eq!(
            request.command,
            Command::TaskBudget {
                item: 7,
                budget: expected
            }
        );
        assert_eq!(serde_json::to_value(request).unwrap(), wire);
    }
}

#[test]
fn a_task_budget_without_its_budget_is_refused_rather_than_lifting_one() {
    for command in [
        json!({"kind": "task-budget", "item": 7}),
        json!({"kind": "task-budget", "item": 7, "budget": null}),
        json!({"kind": "task-budget", "item": 7, "budget": {"minutes": -1}}),
    ] {
        let wire = json!({"id": 5, "command": command});

        let rejection = decode_request(&wire.to_string()).unwrap_err();

        assert_eq!(rejection.error.code, ErrorCode::InvalidRequest, "{command}");
    }
}

#[test]
fn malformed_hex_is_an_invalid_request() {
    for data in ["abc", "zz", "AB"] {
        let wire = json!({
            "id": 3,
            "command": {"kind": "task-image-add", "item": 7, "data": data}
        });

        let rejection = decode_request(&wire.to_string()).unwrap_err();

        assert_eq!(rejection.error.code, ErrorCode::InvalidRequest, "{data}");
    }
}

#[test]
fn a_task_view_says_what_it_waits_for_next() {
    let view = TaskView {
        id: 2,
        project: "app".to_owned(),
        title: "Backfill".to_owned(),
        notes: String::new(),
        kind: TaskKindView::Task,
        priority: 2,
        status: TaskStatusView::Todo,
        is_blocker_report: false,
        why: None,
        filed_by: None,
        filed_from: None,
        doing_session: None,
        summary: None,
        is_queued: false,
        waits: Vec::new(),
        images: Vec::new(),
        intent_revision: 1,
        state_revision: 1,
        created_at: 10,
        changed_at: 10,
        next: TaskNextView::FinishDependencies { items: vec![1] },
        budget_minutes: None,
    };

    let wire = serde_json::to_value(Reply::Task {
        task: Box::new(view.clone()),
    })
    .unwrap();

    assert_eq!(
        wire["task"]["next"],
        json!({"kind": "finish-dependencies", "items": [1]})
    );
    assert_eq!(wire["task"]["status"], json!("todo"));
    let read: Reply = serde_json::from_value(wire).unwrap();
    assert_eq!(
        read,
        Reply::Task {
            task: Box::new(view)
        }
    );
}

#[test]
fn a_status_or_next_action_this_version_does_not_know_reads_as_unknown() {
    let status: TaskStatusView = serde_json::from_value(json!("archived")).unwrap();
    let next: TaskNextView = serde_json::from_value(json!({"kind": "await-review"})).unwrap();

    assert_eq!(
        (status, next),
        (TaskStatusView::Unknown, TaskNextView::Unknown)
    );
}
