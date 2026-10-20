mod common;
use common::sandbox::Sandbox;
use cockpit::teams::{AgentAction, HostAction, Member, MessageKind, Proposal, Role, Submission, TaskKey, TransportOutcome, VerifiedSession, store};
use serde_json::Value;

const DESIGNER: &str = "aaaaaaaa-0000-4000-8000-000000000000";
const DEVELOPER: &str = "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L";
const QA: &str = "bbbbbbbb-0000-4000-8000-000000000000";

fn task() -> TaskKey { TaskKey { project: "project-a".into(), task_id: "1d3fd318".into() } }

fn live() -> Vec<VerifiedSession> {
    [(DESIGNER, "claude"), (DEVELOPER, "opencode"), (QA, "claude")].into_iter()
        .map(|(id, agent)| VerifiedSession { session_id: id.into(), agent: agent.into(), project: "project-a".into(), runtime: format!("runtime-{id}") }).collect()
}

fn proposal() -> Proposal {
    Proposal { submission_key: "link-1".into(), task: task(), reason: "Clarify and review this task".into(), members: vec![
        Member { session_id: DESIGNER.into(), role: Role::Designer },
        Member { session_id: DEVELOPER.into(), role: Role::Developer },
        Member { session_id: QA.into(), role: Role::Qa },
    ] }
}

fn propose(connection: &mut rusqlite::Connection, proposal: &Proposal) -> Value {
    store::propose(connection, &live()[1], proposal, &live(), 1000).unwrap().result.unwrap()
}

fn approve(connection: &mut rusqlite::Connection, request: &Value) -> Result<cockpit::teams::TeamReply, cockpit::teams::TeamError> {
    store::control(connection, &HostAction::Approve { request_id: format!("approval-{}", request["proposal_id"].as_str().unwrap()),
        proposal_id: request["proposal_id"].as_str().unwrap().into(), expected_revision: request["revision"].as_i64().unwrap(),
        mapping_hash: request["mapping_hash"].as_str().unwrap().into() }, &live(), 2000)
}

#[test]
fn pending_request_grants_no_messaging_membership() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    propose(&mut connection, &proposal());
    let state = store::snapshot(&connection, Some(&task()), None, 100).unwrap();
    assert_eq!(state["members"], serde_json::json!([]));
    assert_eq!(state["proposals"][0]["status"], "pending");
}

#[test]
fn only_the_displayed_exact_mapping_can_be_approved() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    let mut request = propose(&mut connection, &proposal());
    let exact_hash = request["mapping_hash"].clone();
    request["mapping_hash"] = serde_json::json!("forged hash");
    assert!(approve(&mut connection, &request).is_err());
    assert_eq!(store::snapshot(&connection, Some(&task()), None, 100).unwrap()["members"], serde_json::json!([]));
    request["mapping_hash"] = exact_hash;
    approve(&mut connection, &request).unwrap();
    assert_eq!(store::snapshot(&connection, Some(&task()), None, 100).unwrap()["members"].as_array().unwrap().len(), 3);
}

#[test]
fn approvals_and_runtime_bindings_survive_reopening_the_database() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("protected/teams.sqlite");
    let mut connection = store::open(&path).unwrap();
    let request = propose(&mut connection, &proposal());
    approve(&mut connection, &request).unwrap();
    drop(connection);
    let connection = store::open(&path).unwrap();
    let state = store::snapshot(&connection, Some(&task()), None, 100).unwrap();
    assert_eq!(state["members"][0]["task_id"], "1d3fd318");
    assert!(state["members"].as_array().unwrap().iter().all(|member| member["runtime"].as_str().unwrap().starts_with("runtime-")));
}

#[test]
fn replacing_a_live_runtime_requires_a_new_request() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    let request = propose(&mut connection, &proposal());
    let mut replaced = live();
    replaced[2].runtime = "replacement-runtime".into();
    let action = HostAction::Approve { request_id: "approval-1".into(), proposal_id: request["proposal_id"].as_str().unwrap().into(),
        expected_revision: request["revision"].as_i64().unwrap(), mapping_hash: request["mapping_hash"].as_str().unwrap().into() };
    assert!(store::control(&mut connection, &action, &replaced, 2000).is_err());
}

#[test]
fn duplicate_proposals_return_the_original_request_but_changed_contents_fail() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    let first = propose(&mut connection, &proposal());
    let duplicate = propose(&mut connection, &proposal());
    assert_eq!(first["proposal_id"], duplicate["proposal_id"]);
    let mut different = proposal();
    different.task.task_id = "0123abcd".into();
    assert!(store::propose(&mut connection, &live()[1], &different, &live(), 1001).is_err());
}

#[test]
fn one_session_cannot_be_approved_into_two_tasks() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    let request = propose(&mut connection, &proposal());
    approve(&mut connection, &request).unwrap();
    let mut other = proposal();
    other.submission_key = "link-2".into();
    other.task.task_id = "0123abcd".into();
    let request = propose(&mut connection, &other);
    assert!(approve(&mut connection, &request).is_err());
}

#[test]
fn unrelated_projects_or_duplicate_role_assignments_cannot_link() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    let mut invalid = proposal();
    invalid.task.project = "project-b".into();
    assert!(store::propose(&mut connection, &live()[1], &invalid, &live(), 1000).is_err());
    invalid = proposal();
    invalid.members[2].role = Role::Developer;
    assert!(store::propose(&mut connection, &live()[1], &invalid, &live(), 1000).is_err());
}

#[test]
fn symlinked_authoritative_state_is_refused() {
    let sandbox = Sandbox::new();
    let target = sandbox.root.join("target");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(target, sandbox.root.join("protected")).unwrap();
    assert!(store::open(&sandbox.root.join("protected/teams.sqlite")).is_err());
}

#[test]
fn editing_a_request_invalidates_the_old_confirmation() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    let original = propose(&mut connection, &proposal());
    let mut replacement = proposal();
    replacement.task.task_id = "0123abcd".into();
    let revised = store::control(&mut connection, &HostAction::Edit { request_id: "edit-1".into(),
        proposal_id: original["proposal_id"].as_str().unwrap().into(), expected_revision: 1, replacement }, &live(), 1500).unwrap().result.unwrap();
    assert!(approve(&mut connection, &original).is_err());
    approve(&mut connection, &revised).unwrap();
    let state = store::snapshot(&connection, None, None, 100).unwrap();
    assert!(state["members"].as_array().unwrap().iter().all(|member| member["task_id"] == "0123abcd"));
}

#[test]
fn rejecting_a_request_never_grants_membership() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    let request = propose(&mut connection, &proposal());
    store::control(&mut connection, &HostAction::Reject { request_id: "reject-1".into(), proposal_id: request["proposal_id"].as_str().unwrap().into(), expected_revision: 1 }, &live(), 1500).unwrap();
    assert!(approve(&mut connection, &request).is_err());
    assert_eq!(store::snapshot(&connection, None, None, 100).unwrap()["members"], serde_json::json!([]));
}

#[test]
fn unlink_revokes_membership_and_old_approval_cannot_restore_it() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    let request = propose(&mut connection, &proposal());
    approve(&mut connection, &request).unwrap();
    store::control(&mut connection, &HostAction::Unlink { request_id: "unlink-1".into(), task: task(), session_id: QA.into() }, &live(), 3000).unwrap();
    approve(&mut connection, &request).unwrap();
    let state = store::snapshot(&connection, Some(&task()), None, 100).unwrap();
    assert_eq!(state["members"].as_array().unwrap().len(), 2);
    assert!(state["members"].as_array().unwrap().iter().all(|member| member["session_id"] != QA));
}

#[test]
fn adding_approved_qa_preserves_the_unchanged_developer_epoch() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    let mut incomplete = proposal();
    incomplete.members.pop();
    let request = propose(&mut connection, &incomplete);
    approve(&mut connection, &request).unwrap();
    let state = store::snapshot(&connection, Some(&task()), None, 100).unwrap();
    let epoch = state["members"].as_array().unwrap().iter().find(|member| member["session_id"] == DEVELOPER).unwrap()["epoch"].clone();
    let mut complete = proposal();
    complete.submission_key = "add-qa".into();
    let request = propose(&mut connection, &complete);
    approve(&mut connection, &request).unwrap();
    let state = store::snapshot(&connection, Some(&task()), None, 100).unwrap();
    assert_eq!(state["members"].as_array().unwrap().len(), 3);
    assert_eq!(state["members"].as_array().unwrap().iter().find(|member| member["session_id"] == DEVELOPER).unwrap()["epoch"], epoch);
}

#[test]
fn unsupported_database_versions_are_not_overwritten() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("protected/teams.sqlite");
    let connection = store::open(&path).unwrap();
    connection.execute_batch("PRAGMA user_version=2").unwrap();
    drop(connection);
    assert!(store::open(&path).is_err());
    assert_eq!(rusqlite::Connection::open(path).unwrap().query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
}

#[test]
fn proposal_limits_survive_restarts_and_duplicate_requests_do_not_consume_them() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("protected/teams.sqlite");
    let mut connection = store::open(&path).unwrap();
    for index in 0..10 {
        let mut request = proposal();
        request.submission_key = format!("request-{index}");
        store::propose(&mut connection, &live()[1], &request, &live(), 1000).unwrap();
        store::propose(&mut connection, &live()[1], &request, &live(), 1000).unwrap();
    }
    drop(connection);
    let mut connection = store::open(&path).unwrap();
    assert!(store::propose(&mut connection, &live()[1], &proposal(), &live(), 1001).is_err());
    assert!(store::propose(&mut connection, &live()[1], &proposal(), &live(), 61_001).is_ok());
}

fn message(key: &str) -> AgentAction {
    AgentAction::Send { message: Submission { task: task(), submission_key: key.into(), recipient: DESIGNER.into(),
        kind: MessageKind::Notice, body: "Please check the approved frame.".into(), thread_id: None, handoff_id: None } }
}

fn linked(connection: &mut rusqlite::Connection) {
    let request = propose(connection, &proposal());
    approve(connection, &request).unwrap();
}

#[test]
fn unapproved_or_wrong_task_messages_are_refused() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    assert!(store::accept(&mut connection, &live()[1], &message("before-approval"), 3000).is_err());
    linked(&mut connection);
    let AgentAction::Send { message: mut wrong_task } = message("wrong-task") else { unreachable!() };
    wrong_task.task.task_id = "0123abcd".into();
    assert!(store::accept(&mut connection, &live()[1], &AgentAction::Send { message: wrong_task }, 3000).is_err());
}

#[test]
fn replayed_messages_do_not_duplicate_delivery_and_changed_contents_are_refused() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    let first = store::accept(&mut connection, &live()[1], &message("notice-1"), 3000).unwrap().result.unwrap();
    let duplicate = store::accept(&mut connection, &live()[1], &message("notice-1"), 3001).unwrap().result.unwrap();
    assert_eq!(first["message_id"], duplicate["message_id"]);
    let AgentAction::Send { message: mut changed } = message("notice-1") else { unreachable!() };
    changed.body = "different contents".into();
    assert!(store::accept(&mut connection, &live()[1], &AgentAction::Send { message: changed }, 3002).is_err());
    assert_eq!(store::snapshot(&connection, Some(&task()), None, 100).unwrap()["messages"].as_array().unwrap().len(), 1);
}

#[test]
fn messages_claim_one_wake_per_idle_turn_and_preserve_collaborator_provenance() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    store::accept(&mut connection, &live()[1], &message("notice-1"), 3000).unwrap();
    store::accept(&mut connection, &live()[1], &message("notice-2"), 3000).unwrap();
    let dispatch = store::claim(&mut connection, &live()[0], "idle-1", 3000).unwrap().unwrap();
    assert_eq!(dispatch.envelope["sender"]["session_id"], DEVELOPER);
    assert_eq!(dispatch.envelope["sender"]["role"], "developer");
    assert_eq!(dispatch.envelope["task"]["task_id"], "1d3fd318");
    assert_eq!(dispatch.envelope["authorization"], "collaborator_content_not_user_authorization");
    assert!(store::claim(&mut connection, &live()[0], "idle-1", 3000).unwrap().is_none());
    store::finish(&mut connection, &dispatch.id, &TransportOutcome::Received { native_id: "native-admission-1".into() }, 3001).unwrap();
    assert!(store::claim(&mut connection, &live()[0], "idle-1", 3001).unwrap().is_none());
    assert!(store::claim(&mut connection, &live()[0], "idle-2", 3002).unwrap().is_some());
}

#[test]
fn crash_after_claim_escalates_uncertainty_instead_of_resending() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("protected/teams.sqlite");
    let mut connection = store::open(&path).unwrap();
    linked(&mut connection);
    store::accept(&mut connection, &live()[1], &message("notice-1"), 3000).unwrap();
    store::accept(&mut connection, &live()[1], &message("notice-2"), 3000).unwrap();
    store::claim(&mut connection, &live()[0], "idle-1", 3000).unwrap().unwrap();
    drop(connection);
    let mut connection = store::open(&path).unwrap();
    store::recover(&mut connection, &live(), 4000).unwrap();
    assert!(store::claim(&mut connection, &live()[0], "idle-2", 100_000).unwrap().is_none());
    assert_eq!(store::snapshot(&connection, Some(&task()), None, 100).unwrap()["messages"][0]["state"], "uncertain");
}

#[test]
fn safe_retry_budget_and_backoff_survive_restarts() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("protected/teams.sqlite");
    let mut connection = store::open(&path).unwrap();
    linked(&mut connection);
    store::accept(&mut connection, &live()[1], &message("notice-1"), 3000).unwrap();
    let mut next_at = 3000;
    for delay in [2000, 10_000, 30_000] {
        let dispatch = store::claim(&mut connection, &live()[0], "idle-1", next_at).unwrap().unwrap();
        store::finish(&mut connection, &dispatch.id, &TransportOutcome::NotSubmitted { reason: "native inbox unavailable".into() }, next_at).unwrap();
        drop(connection);
        connection = store::open(&path).unwrap();
        assert!(store::claim(&mut connection, &live()[0], "idle-1", next_at + delay - 1).unwrap().is_none());
        next_at += delay;
    }
    let dispatch = store::claim(&mut connection, &live()[0], "idle-1", next_at).unwrap().unwrap();
    store::finish(&mut connection, &dispatch.id, &TransportOutcome::NotSubmitted { reason: "still unavailable".into() }, next_at).unwrap();
    assert!(store::claim(&mut connection, &live()[0], "idle-2", next_at + 1_000_000).unwrap().is_none());
    let state = store::snapshot(&connection, Some(&task()), None, 100).unwrap();
    assert_eq!(state["messages"][0]["state"], "failed");
    assert_eq!(state["messages"][0]["attempts"], 4);
}

#[test]
fn unlink_cancels_pending_retries_and_relink_does_not_revive_them() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    store::accept(&mut connection, &live()[1], &message("notice-1"), 3000).unwrap();
    let dispatch = store::claim(&mut connection, &live()[0], "idle-1", 3000).unwrap().unwrap();
    store::finish(&mut connection, &dispatch.id, &TransportOutcome::NotSubmitted { reason: "offline".into() }, 3000).unwrap();
    store::control(&mut connection, &HostAction::Unlink { request_id: "unlink-1".into(), task: task(), session_id: DESIGNER.into() }, &live(), 3001).unwrap();
    let mut fresh = proposal();
    fresh.submission_key = "relink".into();
    let request = propose(&mut connection, &fresh);
    approve(&mut connection, &request).unwrap();
    assert!(store::claim(&mut connection, &live()[0], "idle-2", 10_000).unwrap().is_none());
    assert_eq!(store::snapshot(&connection, Some(&task()), None, 100).unwrap()["messages"][0]["state"], "cancelled");
}

#[test]
fn duplicate_receipts_cannot_reset_the_delivery_or_wake_again() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("protected/teams.sqlite");
    let mut connection = store::open(&path).unwrap();
    linked(&mut connection);
    store::accept(&mut connection, &live()[1], &message("notice-1"), 3000).unwrap();
    let dispatch = store::claim(&mut connection, &live()[0], "idle-1", 3000).unwrap().unwrap();
    store::finish(&mut connection, &dispatch.id, &TransportOutcome::Received { native_id: "native-admission-1".into() }, 3001).unwrap();
    drop(connection);
    let mut connection = store::open(&path).unwrap();
    store::finish(&mut connection, &dispatch.id, &TransportOutcome::Received { native_id: "native-admission-1".into() }, 3002).unwrap();
    assert!(store::claim(&mut connection, &live()[0], "idle-2", 3002).unwrap().is_none());
    assert_eq!(store::snapshot(&connection, Some(&task()), None, 100).unwrap()["messages"][0]["state"], "received");
}

#[test]
fn a_replaced_recipient_runtime_cannot_claim_old_messages() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    store::accept(&mut connection, &live()[1], &message("notice-1"), 3000).unwrap();
    let mut replacement = live()[0].clone();
    replacement.runtime = "new-runtime".into();
    assert!(store::claim(&mut connection, &replacement, "idle-1", 3001).unwrap().is_none());
}

#[test]
fn only_the_addressed_runtime_can_acknowledge_a_delivery() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    store::accept(&mut connection, &live()[1], &message("notice-1"), 3000).unwrap();
    let dispatch = store::claim(&mut connection, &live()[0], "idle-1", 3000).unwrap().unwrap();
    let received = TransportOutcome::Received { native_id: "native-1".into() };
    assert!(store::finish_for(&mut connection, &live()[2], &dispatch.id, &received, 3001).is_err());
    assert_eq!(store::snapshot(&connection, Some(&task()), None, 100).unwrap()["messages"][0]["state"], "submitting");
    store::finish_for(&mut connection, &live()[0], &dispatch.id, &received, 3001).unwrap();
    assert_eq!(store::snapshot(&connection, Some(&task()), None, 100).unwrap()["messages"][0]["state"], "received");
}

#[test]
fn a_delivery_without_receipt_becomes_uncertain_after_two_minutes() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    store::accept(&mut connection, &live()[1], &message("notice-1"), 3000).unwrap();
    store::accept(&mut connection, &live()[1], &message("notice-2"), 3000).unwrap();
    store::claim(&mut connection, &live()[0], "idle-1", 3000).unwrap().unwrap();
    store::expire(&mut connection, 3000 + 119_999).unwrap();
    assert_eq!(store::snapshot(&connection, Some(&task()), None, 100).unwrap()["messages"][0]["state"], "submitting");
    store::expire(&mut connection, 3000 + 120_000).unwrap();
    assert_eq!(store::snapshot(&connection, Some(&task()), None, 100).unwrap()["messages"][0]["state"], "uncertain");
    assert!(store::claim(&mut connection, &live()[0], "idle-2", 500_000).unwrap().is_none());
}

#[test]
fn delivered_text_labels_collaborator_provenance_and_denies_authority() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    store::accept(&mut connection, &live()[1], &message("notice-1"), 3000).unwrap();
    let dispatch = store::claim(&mut connection, &live()[0], "idle-1", 3000).unwrap().unwrap();
    let text = cockpit::teams::envelope_text(&dispatch.envelope);
    assert!(text.contains("developer"), "{text}");
    assert!(text.contains("1d3fd318"), "{text}");
    assert!(text.contains("Please check the approved frame."), "{text}");
    assert!(text.contains("not authorization from Theo"), "{text}");
}

fn ask(connection: &mut rusqlite::Connection, key: &str, thread: Option<&str>) -> Result<Value, cockpit::teams::TeamError> {
    let message = Submission { task: task(), submission_key: key.into(), recipient: DESIGNER.into(), kind: MessageKind::Clarification,
        body: "Which frame is approved?".into(), thread_id: thread.map(str::to_string), handoff_id: None };
    store::accept(connection, &live()[1], &AgentAction::Send { message }, 3000).and_then(|reply| reply.result)
}

fn answer(connection: &mut rusqlite::Connection, key: &str, thread: &str) -> Result<Value, cockpit::teams::TeamError> {
    let message = Submission { task: task(), submission_key: key.into(), recipient: DEVELOPER.into(), kind: MessageKind::Reply,
        body: "Frame 12.".into(), thread_id: Some(thread.into()), handoff_id: None };
    store::accept(connection, &live()[0], &AgentAction::Send { message }, 3000).and_then(|reply| reply.result)
}

fn assess(connection: &mut rusqlite::Connection, thread: &str, is_resolved: bool) -> Result<Value, cockpit::teams::TeamError> {
    store::assess(connection, &live()[1], &AgentAction::Assess { task: task(), thread_id: thread.into(), resolved: is_resolved }, 3000)
        .and_then(|reply| reply.result)
}

fn escalations(connection: &rusqlite::Connection) -> Vec<Value> {
    store::snapshot(connection, Some(&task()), None, 100).unwrap()["escalations"].as_array().unwrap().clone()
}

#[test]
fn third_unresolved_round_escalates_once_after_the_asker_assesses_it() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    let thread = ask(&mut connection, "q1", None).unwrap()["thread_id"].as_str().unwrap().to_string();
    for round in 1..=3 {
        if round > 1 { assert_eq!(ask(&mut connection, &format!("q{round}"), Some(&thread)).unwrap()["round"], round); }
        answer(&mut connection, &format!("a{round}"), &thread).unwrap();
        if round < 3 { assert_eq!(assess(&mut connection, &thread, false).unwrap()["status"], "awaiting_followup"); }
    }
    assert!(escalations(&connection).is_empty(), "the third reply must first be assessed");
    assert_eq!(assess(&mut connection, &thread, false).unwrap()["status"], "escalated");
    assert_eq!(escalations(&connection).len(), 1);
    assert!(assess(&mut connection, &thread, false).is_err());
    assert_eq!(escalations(&connection).len(), 1);
}

#[test]
fn a_fourth_request_is_refused_and_escalates_once() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    let thread = ask(&mut connection, "q1", None).unwrap()["thread_id"].as_str().unwrap().to_string();
    for round in 1..=3 {
        if round > 1 { ask(&mut connection, &format!("q{round}"), Some(&thread)).unwrap(); }
        answer(&mut connection, &format!("a{round}"), &thread).unwrap();
        if round < 3 { assess(&mut connection, &thread, false).unwrap(); }
    }
    assert!(ask(&mut connection, "q4", Some(&thread)).is_err());
    assert!(ask(&mut connection, "q5", Some(&thread)).is_err());
    assert_eq!(escalations(&connection).len(), 1);
}

#[test]
fn a_resolved_thread_does_not_escalate_or_accept_more_requests() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    let thread = ask(&mut connection, "q1", None).unwrap()["thread_id"].as_str().unwrap().to_string();
    answer(&mut connection, "a1", &thread).unwrap();
    assert_eq!(assess(&mut connection, &thread, true).unwrap()["status"], "resolved");
    assert!(ask(&mut connection, "q2", Some(&thread)).is_err());
    assert!(escalations(&connection).is_empty());
}

#[test]
fn duplicate_replies_and_transport_retries_do_not_advance_rounds() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    let thread = ask(&mut connection, "q1", None).unwrap()["thread_id"].as_str().unwrap().to_string();
    let dispatch = store::claim(&mut connection, &live()[0], "idle-1", 3000).unwrap().unwrap();
    store::finish(&mut connection, &dispatch.id, &TransportOutcome::NotSubmitted { reason: "busy".into() }, 3000).unwrap();
    answer(&mut connection, "a1", &thread).unwrap();
    answer(&mut connection, "a1", &thread).unwrap();
    assess(&mut connection, &thread, false).unwrap();
    assert_eq!(ask(&mut connection, "q2", Some(&thread)).unwrap()["round"], 2);
}

#[test]
fn a_delivered_clarification_names_its_thread_and_round() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    let thread = ask(&mut connection, "q1", None).unwrap()["thread_id"].as_str().unwrap().to_string();
    let dispatch = store::claim(&mut connection, &live()[0], "idle-1", 3000).unwrap().unwrap();
    let text = cockpit::teams::envelope_text(&dispatch.envelope);
    assert!(text.contains(&format!("Thread: {thread} (round 1 of 3)")), "{text}");
}

#[test]
fn independent_questions_have_independent_round_counters() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    let first = ask(&mut connection, "q1", None).unwrap();
    let second = ask(&mut connection, "q2", None).unwrap();
    assert_ne!(first["thread_id"], second["thread_id"]);
    assert_eq!(second["round"], 1);
}

#[test]
fn only_the_answerer_replies_and_only_the_asker_assesses() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    let thread = ask(&mut connection, "q1", None).unwrap()["thread_id"].as_str().unwrap().to_string();
    let forged = Submission { task: task(), submission_key: "forged".into(), recipient: DESIGNER.into(), kind: MessageKind::Reply,
        body: "self answer".into(), thread_id: Some(thread.clone()), handoff_id: None };
    assert!(store::accept(&mut connection, &live()[1], &AgentAction::Send { message: forged }, 3000).is_err());
    answer(&mut connection, "a1", &thread).unwrap();
    let by_answerer = AgentAction::Assess { task: task(), thread_id: thread.clone(), resolved: true };
    assert!(store::assess(&mut connection, &live()[0], &by_answerer, 3000).is_err());
}

#[test]
fn only_theo_can_close_an_escalation() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    let thread = ask(&mut connection, "q1", None).unwrap()["thread_id"].as_str().unwrap().to_string();
    for round in 1..=3 {
        if round > 1 { ask(&mut connection, &format!("q{round}"), Some(&thread)).unwrap(); }
        answer(&mut connection, &format!("a{round}"), &thread).unwrap();
        assess(&mut connection, &thread, false).unwrap();
    }
    assert!(serde_json::from_value::<AgentAction>(serde_json::json!({"action": "resolve", "thread_id": thread})).is_err());
    store::control(&mut connection, &HostAction::Resolve { request_id: "resolve-1".into(), thread_id: thread.clone(),
        note: "Use frame 12.".into() }, &live(), 4000).unwrap();
    assert_eq!(escalations(&connection)[0]["status"], "resolved");
}

#[test]
fn replaced_sender_runtime_invalidates_pending_messages() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    store::accept(&mut connection, &live()[1], &message("notice-1"), 3000).unwrap();
    let mut replacements = live();
    replacements[1].runtime = "replacement-runtime".into();
    store::recover(&mut connection, &replacements, 3001).unwrap();
    assert!(store::claim(&mut connection, &live()[0], "idle-1", 3001).unwrap().is_none());
    assert_eq!(store::snapshot(&connection, Some(&task()), None, 100).unwrap()["messages"][0]["state"], "cancelled");
}

#[test]
fn unicode_message_limit_is_characters_not_bytes_and_controls_are_refused() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    for (body, is_valid) in [("é".repeat(4000), true), ("é".repeat(4001), false), ("hello\u{1b}[31m".into(), false)] {
        let AgentAction::Send { message: mut notice } = message(if is_valid { "valid" } else { "invalid" }) else { unreachable!() };
        notice.body = body;
        assert_eq!(store::accept(&mut connection, &live()[1], &AgentAction::Send { message: notice }, 3000).is_ok(), is_valid);
    }
}

#[test]
fn a_team_cannot_accumulate_more_than_one_hundred_pending_messages() {
    let sandbox = Sandbox::new();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    linked(&mut connection);
    for index in 0..100 {
        store::accept(&mut connection, &live()[1], &message(&format!("notice-{index}")), 100_000 + index * 60_001).unwrap();
    }
    assert!(store::accept(&mut connection, &live()[1], &message("notice-over-limit"), 10_000_000).is_err());
    assert_eq!(store::snapshot(&connection, Some(&task()), None, 100).unwrap()["messages"].as_array().unwrap().len(), 100);
}

#[test]
fn concurrent_claims_do_not_duplicate_native_wake_attempts() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("protected/teams.sqlite");
    let mut connection = store::open(&path).unwrap();
    linked(&mut connection);
    store::accept(&mut connection, &live()[1], &message("notice-1"), 3000).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2).map(|_| {
        let (path, barrier) = (path.clone(), barrier.clone());
        std::thread::spawn(move || {
            let mut connection = store::open(&path).unwrap();
            barrier.wait();
            store::claim(&mut connection, &live()[0], "idle-1", 3000)
        })
    }).collect();
    let outcomes: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|result| matches!(result, Ok(Some(_)))).count(), 1);
    let state = store::snapshot(&connection, Some(&task()), None, 100).unwrap();
    assert_eq!(state["messages"][0]["attempts"], 1);
    assert_eq!(state["messages"][0]["state"], "submitting");
}
