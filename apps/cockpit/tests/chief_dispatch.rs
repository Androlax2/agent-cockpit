use cockpit::chief::{store, ChiefHostAction, WorkerState};
use cockpit::teams::{TaskKey, VerifiedSession};
use rusqlite::Connection;
mod common;
use common::sandbox::Sandbox;

const NOW: i64 = 100_000;

#[test]
fn unknown_legacy_launch_keeps_ownership_until_exact_host_safe_stop() {
    let mut connection=Connection::open_in_memory().unwrap();store::migrate(&mut connection).unwrap();
    let launch=task("aaaaaaaa");
    let token=store::reserve_legacy(&mut connection,&launch,None,NOW).unwrap();
    store::observe(&mut connection,&[],NOW+1).unwrap();
    let page=store::snapshot(&connection,&[],0,50).unwrap();
    assert!(page["records"].as_array().unwrap().iter().any(|record|record["kind"]=="legacy_launch" && record["id"]==token),"unknown launch must remain visibly reconcilable with Chief never enabled");
    let action:ChiefHostAction=serde_json::from_value(serde_json::json!({"action":"reconcile_launch","request_id":"stop-unknown","token":token,"expected_revision":1,"evidence":{"kind":"safe_stop","note":"Verified exact launch cannot execute","confirmed_stopped":true}})).unwrap();
    store::host(&mut connection,&action,&[],NOW+2).unwrap().result.unwrap();
    store::reserve_legacy(&mut connection,&launch,None,NOW+3).unwrap();
}

#[test]
fn disabling_a_chief_leaves_legacy_reservations_available() {
    let (mut connection, live) = setup();
    store::reserve_legacy(&mut connection, &task("aaaaaaaa"), Some(&session('b').session_id), NOW).unwrap();
    let epoch: String = connection.query_row("SELECT epoch FROM chief_grants", [], |row| row.get(0)).unwrap();
    store::host(&mut connection, &ChiefHostAction::Disable { request_id: "disable".into(), expected_epoch: epoch }, &live, NOW).unwrap().result.unwrap();
    assert!(store::reserve_legacy(&mut connection, &task("aaaaaaaa"), Some(&session('c').session_id), NOW).is_err(), "the earlier reservation is not the chief's to drop");
    store::reserve_legacy(&mut connection, &task("cccccccc"), Some(&session('c').session_id), NOW).unwrap();
}

#[test]
fn an_active_chief_does_not_pause_legacy_reservations() {
    for state in ["active", "paused"] {
        let (mut connection, _) = setup();
        connection.execute("UPDATE chief_grants SET state=?1", [state]).unwrap();
        store::reserve_legacy(&mut connection, &task("cccccccc"), None, NOW).unwrap();
    }
}

#[test]
fn two_legacy_clients_cannot_reserve_the_same_task() {
    use std::sync::{Arc, Barrier};
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("legacy.sqlite");
    let mut connection = Connection::open(&path).unwrap();
    store::migrate(&mut connection).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let threads = (0..2).map(|_| {
        let path = path.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            let mut connection = Connection::open(path).unwrap();
            connection.busy_timeout(std::time::Duration::from_secs(3)).unwrap();
            barrier.wait();
            store::reserve_legacy(&mut connection, &task("aaaaaaaa"), None, NOW).is_ok()
        })
    }).collect::<Vec<_>>();
    assert_eq!(threads.into_iter().map(|thread| thread.join().unwrap()).filter(|reserved| *reserved).count(), 1);
}

#[test]
fn an_unknown_legacy_launch_reserves_its_project_until_the_worker_is_known() {
    let (mut connection, _) = setup();
    store::reserve_legacy(&mut connection, &task("cccccccc"), None, NOW).unwrap();
    assert!(store::reserve_legacy(&mut connection, &task("dddddddd"), None, NOW).is_err());
    assert!(store::reserve_legacy(&mut connection, &task("dddddddd"), Some(&session('b').session_id), NOW).is_err());
}

#[test]
fn a_doing_issue_without_a_session_does_not_block_reservations_in_other_projects() {
    let (mut connection, _) = setup();
    let mut todos = cockpit::todo::TodoList::default();
    todos.issues.insert("org/elsewhere#50".into(), serde_json::json!({"status":"doing","session":""}));
    store::reconcile_todos(&mut connection, &todos, NOW).unwrap();
    store::reserve_legacy(&mut connection, &task("aaaaaaaa"), Some(&session('b').session_id), NOW).unwrap();
}

#[test]
fn a_doing_todo_without_a_session_reserves_only_its_own_task() {
    let (mut connection, _) = setup();
    let mut todos = cockpit::todo::TodoList::default();
    todos.items.push(cockpit::todo::Item(serde_json::json!({"id":"cccccccc", "text":"started elsewhere", "cwd":"/project-a",
        "status":"doing"}).as_object().unwrap().clone()));
    store::reconcile_todos(&mut connection, &todos, NOW).unwrap();
    store::reserve_legacy(&mut connection, &task("aaaaaaaa"), Some(&session('b').session_id), NOW).unwrap();
    assert!(store::reserve_legacy(&mut connection, &task("cccccccc"), Some(&session('c').session_id), NOW).is_err());
}

fn session(letter: char) -> VerifiedSession {
    if matches!(letter,'a'|'d') {
        return VerifiedSession {session_id:format!("ses_{letter}00000000000000000000000000"),agent:"opencode".into(),project:"path:/project-a".into(),runtime:format!("runtime-{letter}")};
    }
    VerifiedSession { session_id: format!("{0}{0}{0}{0}{0}{0}{0}{0}-{0}{0}{0}{0}-{0}{0}{0}{0}-{0}{0}{0}{0}-{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}", letter),
        agent: "claude".into(), project: "path:/project-a".into(), runtime: format!("runtime-{letter}") }
}

fn task(task_id: &str) -> TaskKey {
    TaskKey { project: "path:/project-a".into(), task_id: task_id.into() }
}

fn setup() -> (Connection, Vec<WorkerState>) {
    let mut connection = Connection::open_in_memory().unwrap();
    store::migrate(&mut connection).unwrap();
    let live = ['a', 'b', 'c'].into_iter().map(|letter| WorkerState { session: session(letter), role: None, ready: true,
        blocked_by: vec![], observed_at_ms: NOW }).collect::<Vec<_>>();
    store::host(&mut connection, &ChiefHostAction::Enable { request_id: "enable".into(), session: session('a') }, &live, NOW).unwrap().result.unwrap();
    (connection, live)
}

#[test]
fn chief_cli_uses_nested_agent_json_without_exposing_host_actions_on_agent_socket() {
    use std::{io::Write, process::Stdio};
    let sandbox = Sandbox::new();
    let requests = sandbox.team_socket(serde_json::Value::Null);
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("chief")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(br#"{"action":"chief","request":{"action":"context","after":0,"limit":10}}"#).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(requests.lock().unwrap()[0]["request"]["action"], "context");
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("chief")
        .stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(br#"{"request":"chief","action":{"action":"disable","request_id":"no","expected_epoch":"epoch"}}"#).unwrap();
    assert!(!child.wait_with_output().unwrap().status.success());
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[test]
fn chief_host_cli_uses_host_socket_and_never_falls_back_to_agent_authority() {
    use std::{io::Write, process::Stdio};
    let sandbox = Sandbox::new();
    let requests = sandbox.team_socket(serde_json::Value::Null);
    let host_dir = sandbox.runtime.join("claude-team-host");
    std::fs::create_dir_all(&host_dir).unwrap();
    std::fs::hard_link(sandbox.answers("teams").join("agent.sock"), host_dir.join("host.sock")).unwrap();
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("chief-host")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(br#"{"request":"chief_snapshot","after":0,"limit":10}"#).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(requests.lock().unwrap()[0]["request"], "chief_snapshot");
    std::fs::remove_file(host_dir.join("host.sock")).unwrap();
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("chief-host")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(br#"{"request":"chief_snapshot","after":0,"limit":10}"#).unwrap();
    assert!(!child.wait_with_output().unwrap().status.success());
    assert_eq!(requests.lock().unwrap().len(), 1);
}
