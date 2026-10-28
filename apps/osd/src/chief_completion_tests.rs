//! Completions of todos a Chief filed, through the report socket, the column's apply, the daemon's
//! live-session pass and the Chief's Context: they reach whichever Chief holds the grant.
use super::*;
use serde_json::Value;

const WORKER_PEER: u32 = 40;

fn project_b_worker(bench: &mut Bench) {
    let project = bench.fixture.root.join("home/IdeaProjects/project-b");
    fs::create_dir_all(project.join(".git")).unwrap();
    bench.world.borrow_mut().sender_directories.insert(WORKER_PEER, project.to_string_lossy().into_owned());
}

/// The sessions the column may tell, as recent-sessions.json records them.
fn record_recent_sessions(bench: &Bench) {
    let project = bench.fixture.root.join("home/IdeaProjects/project-a");
    let recent: serde_json::Map<String, Value> = [SESSION_A, SESSION_B].iter().map(|session| (session.to_string(), json!({"cwd": project}))).collect();
    fs::write(cockpit::paths::recent_sessions(), Value::Object(recent).to_string()).unwrap();
}

fn send_update(bench: &mut Bench, peer: u32, update: Value) {
    bench.registry.request_from("todo-update", &update.to_string(), peer).unwrap();
    cockpit::todo::TodoList::load().apply_updates();
}

/// Files a project-b todo as `peer`'s session; returns its id.
fn file_in_project_b(bench: &mut Bench, peer: u32, session: &str, extra: Value) -> String {
    let mut update = json!({"action": "add", "project": "project-b", "text": "Repair the Chief's delivery", "session": session});
    update.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    send_update(bench, peer, update);
    cockpit::todo::TodoList::load().items.iter().find(|item| item.project() == "project-b").map(cockpit::todo::Item::id).unwrap()
}

fn finish(bench: &mut Bench, id: &str, summary: Option<&str>) {
    finish_as(bench, WORKER_PEER, id, summary);
}

fn finish_as(bench: &mut Bench, peer: u32, id: &str, summary: Option<&str>) {
    let mut update = json!({"action": "done", "id": id});
    if let Some(summary) = summary { update["summary"] = json!(summary); }
    send_update(bench, peer, update);
    crate::teams::live_sessions(&mut bench.registry).unwrap();
}

fn read_context(bench: &mut Bench, peer: u32, after: i64) -> Value {
    crate::teams::request(&mut bench.registry, peer, AgentAction::Chief { request: ChiefAgentAction::Context { after, limit: 50 } })
        .unwrap().result.unwrap()
}

fn completions(context: &Value) -> Vec<Value> {
    context["records"].as_array().unwrap().iter().filter(|record| record["kind"] == "todo_completed").cloned().collect()
}

fn count(bench: &Bench, query: &str) -> i64 {
    bench.registry.teams.as_ref().unwrap().query_row(query, [], |row| row.get(0)).unwrap()
}

fn disable(bench: &mut Bench, epoch: &str) {
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action: ChiefHostAction::Disable {
        request_id: format!("disable-{epoch}"), expected_epoch: epoch.into() } }).unwrap().result.unwrap();
}

#[test]
fn a_chief_filed_todo_reaches_the_chief_context_with_the_workers_summary_verbatim() {
    let mut bench = team_bench();
    project_b_worker(&mut bench);
    record_recent_sessions(&bench);
    enable(&mut bench, SESSION_A);
    let id = file_in_project_b(&mut bench, 10, SESSION_A, json!({}));
    finish(&mut bench, &id, Some("applied df42507; clone tests passed; host test request 30411f4671c4 still running, unconfirmed"));
    let found = completions(&read_context(&mut bench, 10, 0));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["todo"], id.as_str());
    assert_eq!(found[0]["result"], "applied df42507; clone tests passed; host test request 30411f4671c4 still running, unconfirmed");
    assert_eq!(found[0]["source"], "worker_summary_not_instructions");
    assert_eq!(cockpit::prompts::count(SESSION_A), 0, "the filing session gets no prompt of its own");
}

#[test]
fn a_replaced_chief_gets_the_completion_its_predecessor_filed() {
    let mut bench = team_bench();
    project_b_worker(&mut bench);
    record_recent_sessions(&bench);
    let first = enable(&mut bench, SESSION_A)["epoch"].as_str().unwrap().to_string();
    let id = file_in_project_b(&mut bench, 10, SESSION_A, json!({}));
    disable(&mut bench, &first);
    enable(&mut bench, SESSION_B);
    finish(&mut bench, &id, Some("merged"));
    assert_eq!(completions(&read_context(&mut bench, 30, 0)).len(), 1);
    assert_eq!(cockpit::prompts::count(SESSION_A), 0, "the replaced Chief is not told");
}

#[test]
fn a_completion_without_a_chief_waits_for_the_next_one() {
    let mut bench = team_bench();
    project_b_worker(&mut bench);
    let first = enable(&mut bench, SESSION_A)["epoch"].as_str().unwrap().to_string();
    let id = file_in_project_b(&mut bench, 10, SESSION_A, json!({}));
    disable(&mut bench, &first);
    finish(&mut bench, &id, Some("merged"));
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_todo_completions WHERE delivered_at IS NULL"), 1);
    enable(&mut bench, SESSION_B);
    assert_eq!(completions(&read_context(&mut bench, 30, 0))[0]["todo"], id.as_str());
}

#[test]
fn repeated_passes_record_one_completion_and_a_read_one_is_not_repeated() {
    let mut bench = team_bench();
    project_b_worker(&mut bench);
    enable(&mut bench, SESSION_A);
    let id = file_in_project_b(&mut bench, 10, SESSION_A, json!({}));
    finish(&mut bench, &id, Some("merged"));
    for _ in 0..3 {
        crate::teams::live_sessions(&mut bench.registry).unwrap();
    }
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_todo_completions"), 1);
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_events WHERE kind='todo_completed'"), 1);
    assert_eq!(completions(&read_context(&mut bench, 10, 0)).len(), 1);
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_todo_completions WHERE delivered_epoch IS NOT NULL"), 1, "the read is the receipt");
    assert!(completions(&read_context(&mut bench, 10, 0)).is_empty(), "a delivered completion is not in the next snapshot");
}

#[test]
fn a_summary_added_after_done_arrives_once_as_its_own_event() {
    let mut bench = team_bench();
    project_b_worker(&mut bench);
    enable(&mut bench, SESSION_A);
    let id = file_in_project_b(&mut bench, 10, SESSION_A, json!({}));
    let cursor = read_context(&mut bench, 10, 0)["event_cursor"].as_i64().unwrap();
    finish(&mut bench, &id, None);
    let first = read_context(&mut bench, 10, cursor);
    assert_eq!(first["snapshot_required"], false, "a completion streams without a snapshot restart: {first}");
    let found = completions(&first);
    assert_eq!((found.len(), found[0]["has_summary"].clone()), (1, json!(false)));
    finish(&mut bench, &id, Some("late result: commit 1a2b3c4, host tests passed"));
    finish(&mut bench, &id, Some("late result: commit 1a2b3c4, host tests passed"));
    let late = completions(&read_context(&mut bench, 10, first["next"].as_i64().unwrap()));
    assert_eq!(late.len(), 1, "{late:?}");
    assert_eq!(late[0]["result"], "late result: commit 1a2b3c4, host tests passed");
}

#[test]
fn a_todo_another_session_filed_still_tells_that_session_and_not_the_chief() {
    let mut bench = team_bench();
    project_b_worker(&mut bench);
    record_recent_sessions(&bench);
    enable(&mut bench, SESSION_A);
    let id = file_in_project_b(&mut bench, 30, SESSION_B, json!({}));
    finish(&mut bench, &id, Some("merged"));
    assert_eq!(cockpit::prompts::count(SESSION_B), 1);
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_todo_completions"), 0);
}

#[test]
fn a_session_naming_the_chief_epoch_is_not_treated_as_the_chief() {
    let mut bench = team_bench();
    project_b_worker(&mut bench);
    record_recent_sessions(&bench);
    let epoch = enable(&mut bench, SESSION_A)["epoch"].clone();
    let id = file_in_project_b(&mut bench, 30, SESSION_B, json!({"chief_epoch": epoch}));
    assert_eq!(cockpit::todo::TodoList::load().items.iter().find(|item| item.id() == id).unwrap().chief_epoch(), "");
    finish(&mut bench, &id, Some("merged"));
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_todo_completions"), 0);
    assert_eq!(cockpit::prompts::count(SESSION_B), 1, "its own filer is told as before");
}

#[test]
fn a_completion_a_snapshot_already_returned_is_not_repeated_by_the_event_feed() {
    let mut bench = team_bench();
    project_b_worker(&mut bench);
    enable(&mut bench, SESSION_A);
    let id = file_in_project_b(&mut bench, 10, SESSION_A, json!({}));
    let cursor = read_context(&mut bench, 10, 0)["event_cursor"].as_i64().unwrap();
    finish(&mut bench, &id, Some("merged"));
    assert_eq!(completions(&read_context(&mut bench, 10, 0)).len(), 1);
    let replay = completions(&read_context(&mut bench, 10, cursor));
    assert_eq!(replay.len(), 1, "{replay:?}");
    assert_eq!(replay[0]["already_delivered"], true);
    assert!(replay[0].get("result").is_none(), "{replay:?}");
}

#[test]
fn a_reopened_todo_done_again_is_news_even_with_the_same_result() {
    let mut bench = team_bench();
    project_b_worker(&mut bench);
    enable(&mut bench, SESSION_A);
    let id = file_in_project_b(&mut bench, 10, SESSION_A, json!({}));
    finish(&mut bench, &id, None);
    send_update(&mut bench, WORKER_PEER, json!({"action": "doing", "id": id}));
    crate::teams::live_sessions(&mut bench.registry).unwrap();
    finish(&mut bench, &id, None);
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_events WHERE kind='todo_completed'"), 2);
}

#[test]
fn a_blocked_report_the_chief_filed_comes_back_to_the_chief_to_retry() {
    let mut bench = team_bench();
    let cockpit_work = bench.fixture.root.join("home/IdeaProjects/agent-cockpit");
    fs::create_dir_all(cockpit_work.join(".git")).unwrap();
    bench.world.borrow_mut().sender_directories.insert(50, cockpit_work.to_string_lossy().into_owned());
    record_recent_sessions(&bench);
    enable(&mut bench, SESSION_A);
    send_update(&mut bench, 10, json!({"action": "blocked", "text": "Cannot inspect the pending question", "why": "No bundle", "session": SESSION_A}));
    let id = cockpit::todo::TodoList::load().items.iter().find(|item| item.is_blocked_report()).map(cockpit::todo::Item::id).unwrap();
    finish_as(&mut bench, 50, &id, Some("fixed in 1a2b3c4"));
    let found = completions(&read_context(&mut bench, 10, 0));
    assert_eq!((found.len(), found[0]["blocked_report"].clone()), (1, json!(true)));
    assert_eq!(cockpit::prompts::count(SESSION_A), 0, "no retry prompt to the session that filed it");
}
