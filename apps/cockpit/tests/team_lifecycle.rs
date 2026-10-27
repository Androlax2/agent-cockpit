use cockpit::teams::{ErrorCode, Recruit, Recruitment, Role, TaskKey, VerifiedSession, store};
use cockpit::todo::{Item, TodoList};
use serde_json::json;
use std::collections::HashMap;

#[path = "common/sandbox.rs"]
mod sandbox;

fn session() -> VerifiedSession {
    VerifiedSession { session_id: "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L".into(), agent: "opencode".into(),
        project: "path:/project-a".into(), runtime: "original-runtime".into() }
}

fn request(task_id: &str) -> Recruitment {
    Recruitment { submission_key: format!("team-{}", task_id.replace(['/', '#'], "-")),
        task: TaskKey { project: "path:/project-a".into(), task_id: task_id.into() }, role: Role::Designer,
        reason: "Build the approved design".into(), recruits: vec![
            Recruit { role: Role::Developer, brief: "Build it".into() },
            Recruit { role: Role::Qa, brief: "Review it".into() }] }
}

fn recruit(connection: &mut store::Connection, sender: &VerifiedSession, request: &Recruitment) -> Result<(), cockpit::teams::TeamError> {
    store::recruit(connection, sender, request, Some("opencode"), &[sender.clone()],
        &HashMap::from([(sender.session_id.clone(), "/project-a".into())]), 1000)?.result?;
    Ok(())
}

fn setup(task_id: &str) -> store::Connection {
    let mut connection = store::Connection::open_in_memory().unwrap();
    store::migrate(&mut connection).unwrap();
    recruit(&mut connection, &session(), &request(task_id)).unwrap();
    connection
}

fn todos(task_id: &str, status: &str) -> TodoList {
    let mut todos = TodoList::default();
    if task_id.contains('#') {
        todos.issues.insert(task_id.into(), json!({"status":status}));
    } else {
        todos.items.push(Item(json!({"id":task_id,"cwd":"/project-a","status":status}).as_object().unwrap().clone()));
    }
    todos
}

#[test]
fn completed_todos_release_stale_runtime_membership_for_new_recruitment() {
    for task_id in ["014f67c1", "org/project#42"] {
        let mut connection = setup(task_id);
        let mut sender = session();
        sender.runtime = "restarted-runtime".into();
        assert!(store::context(&connection, &sender).unwrap()["task"].is_null());
        for now in [2000, 2001] {
            cockpit::chief::store::reconcile_todos(&mut connection, &todos(task_id, "done"), now).unwrap();
        }
        recruit(&mut connection, &sender, &request("1f838f53")).unwrap();
        assert_eq!(store::context(&connection, &sender).unwrap()["task"]["task_id"], "1f838f53");
    }
}

#[test]
fn completing_work_in_a_parallel_clone_releases_its_canonical_team() {
    if std::env::var_os("CLONE_TEAM_LIFECYCLE_CHILD").is_none() {
        let sandbox = sandbox::Sandbox::new();
        let output = sandbox.command(std::env::current_exe().unwrap(), &[("CLONE_TEAM_LIFECYCLE_CHILD", "1")])
            .args(["--exact", "completing_work_in_a_parallel_clone_releases_its_canonical_team", "--nocapture"]).output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    let primary = cockpit::paths::home().join("IdeaProjects/project-a");
    let clone = cockpit::paths::clones_dir().join("project-a/todo-014f67c1");
    let unrelated = cockpit::paths::home().join("unrelated/project-a");
    for directory in [&primary, &clone, &unrelated] {
        std::fs::create_dir_all(directory.join(".git")).unwrap();
    }
    for checkout in [&clone, &primary.join("src"), &unrelated] {
        std::fs::create_dir_all(checkout).unwrap();
        let mut sender = session();
        sender.project = format!("path:{}", primary.display());
        let mut original = request("014f67c1");
        original.task.project = sender.project.clone();
        let mut connection = store::Connection::open_in_memory().unwrap();
        store::migrate(&mut connection).unwrap();
        recruit(&mut connection, &sender, &original).unwrap();
        sender.runtime = "restarted-runtime".into();
        assert!(store::context(&connection, &sender).unwrap()["task"].is_null());
        let mut completed = todos("014f67c1", "done");
        completed.items[0].0.insert("cwd".into(), json!(checkout));
        cockpit::chief::store::reconcile_todos(&mut connection, &completed, 2000).unwrap();
        let mut next = request("1f838f53");
        next.task.project = sender.project.clone();
        if checkout == &unrelated {
            assert_eq!(recruit(&mut connection, &sender, &next).unwrap_err().code, ErrorCode::Conflict);
        } else {
            recruit(&mut connection, &sender, &next).unwrap();
            assert_eq!(store::context(&connection, &sender).unwrap()["task"]["task_id"], "1f838f53");
        }
    }
}

#[test]
fn reconciliation_preserves_active_missing_and_other_project_memberships() {
    for status in ["todo", "doing", "missing", "other-project"] {
        let mut connection = setup("014f67c1");
        let before = store::snapshot(&connection, None, None, 100).unwrap();
        let mut observed = todos("014f67c1", status);
        if status == "missing" { observed.items.clear(); }
        if status == "other-project" {
            observed.items[0].0.insert("cwd".into(), json!("/project-b"));
            observed.items[0].0.insert("status".into(), json!("done"));
        }
        cockpit::chief::store::reconcile_todos(&mut connection, &observed, 2000).unwrap();
        for runtime in ["original-runtime", "restarted-runtime"] {
            let mut sender = session();
            sender.runtime = runtime.into();
            assert_eq!(recruit(&mut connection, &sender, &request("1f838f53")).unwrap_err().code, ErrorCode::Conflict, "{status}");
        }
        assert_eq!(store::snapshot(&connection, None, None, 100).unwrap()["members"], before["members"], "{status}");
    }
}

#[test]
fn completed_todos_cancel_unbound_recruits_before_they_can_restore_membership() {
    for has_started in [false, true] {
        let mut connection = setup("014f67c1");
        let spawn = store::pending_spawns(&connection).unwrap().remove(0);
        if has_started { store::mark_spawn_started(&connection, &spawn.token, 1100).unwrap(); }
        cockpit::chief::store::reconcile_todos(&mut connection, &todos("014f67c1", "done"), 2000).unwrap();
        assert!(store::pending_spawns(&connection).unwrap().is_empty());
        let mut developer = session();
        developer.session_id = "ses_3f2a9c0b1ffeQkTz8mWn4rJd2M".into();
        assert!(!store::bind_spawn(&mut connection, &spawn.token, &developer, 3000).unwrap());
        let snapshot = store::snapshot(&connection, None, None, 100).unwrap();
        assert!(snapshot["members"].as_array().unwrap().is_empty());
        assert!(snapshot["qa_plans"].as_array().unwrap().is_empty());
    }
}

#[test]
fn completing_a_todo_revokes_pending_approval_to_rejoin_its_team() {
    let mut connection = setup("014f67c1");
    let sender = session();
    let proposal = cockpit::teams::Proposal { submission_key: "pending-link".into(), task: request("014f67c1").task,
        reason: "Link the designer".into(), members: vec![cockpit::teams::Member { session_id: sender.session_id.clone(), role: Role::Designer }] };
    let pending = store::propose(&mut connection, &sender, &proposal, &[sender.clone()], 1000).unwrap().result.unwrap();
    cockpit::chief::store::reconcile_todos(&mut connection, &todos("014f67c1", "done"), 2000).unwrap();
    let approval = cockpit::teams::HostAction::Approve { request_id: "approve-old-team".into(),
        proposal_id: pending["proposal_id"].as_str().unwrap().into(), expected_revision: 1,
        mapping_hash: pending["mapping_hash"].as_str().unwrap().into() };
    assert_eq!(store::control(&mut connection, &approval, &[sender], 3000).unwrap_err().code, ErrorCode::Conflict);
    assert!(store::snapshot(&connection, None, None, 100).unwrap()["members"].as_array().unwrap().is_empty());
}
