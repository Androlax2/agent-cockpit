mod common;
#[path = "common/link_fixture.rs"]
mod link_fixture;
#[path = "common/link_history.rs"]
mod link_history;
use common::sandbox::Sandbox;
use cockpit::{teams::{self, TaskKey, links, store}, todo};
use rusqlite::Connection;
use rusqlite::Connection as LinkConnection;
use serde_json::json;
use std::{fs, path::Path};
use cockpit::teams::{AgentAction, ErrorCode, HostAction, LinkProposal, Member, MessageKind, Proposal, Role, Submission, VerifiedSession};
use serde_json::Value;

fn proposal_fixture() -> (Connection, todo::TodoList, Vec<VerifiedSession>, LinkProposal) {
    let first = checkout(&cockpit::paths::home().join("IdeaProjects/first"));
    let second = checkout(&cockpit::paths::home().join("IdeaProjects/second"));
    let mut todos = todo::TodoList::load();
    let task = TaskKey { project: format!("path:{first}"), task_id: todos.add("first", &first, "Joint work") };
    let live = vec![
        VerifiedSession { session_id: "aaaaaaaa-0000-4000-8000-000000000000".into(), agent: "claude".into(), project: task.project.clone(), runtime: "first-runtime".into() },
        VerifiedSession { session_id: "bbbbbbbb-0000-4000-8000-000000000000".into(), agent: "claude".into(), project: format!("path:{second}"), runtime: "second-runtime".into() },
        VerifiedSession { session_id: "cccccccc-0000-4000-8000-000000000000".into(), agent: "claude".into(), project: task.project.clone(), runtime: "third-runtime".into() },
    ];
    let proposal = LinkProposal { submission_key: "link-request".into(), task, peer_session_id: live[1].session_id.clone(), purpose: "Coordinate shared work".into() };
    (store::open(&cockpit::paths::team_database()).unwrap(), todos, live, proposal)
}

fn host_action(value: Value) -> HostAction { serde_json::from_value(value).unwrap() }

fn link_approval(connection: &Connection, receipt: &Value, request: &str) -> HostAction {
    let hash: String = connection.query_row("SELECT mapping_hash FROM link_proposals WHERE id=?1", [receipt["proposal_id"].as_str().unwrap()], |row| row.get(0)).unwrap();
    host_action(json!({"action":"link_approve","request_id":request,"proposal_id":receipt["proposal_id"],"expected_revision":receipt["revision"],"mapping_hash":hash}))
}

fn own_teams(connection: &mut Connection, live: &[VerifiedSession], proposal: &LinkProposal) {
    for (index, developer) in live[..2].iter().enumerate() {
        let mut members = vec![Member { session_id: developer.session_id.clone(), role: Role::Developer }];
        if index == 0 { members.push(Member { session_id: live[2].session_id.clone(), role: Role::Designer }); }
        let request = Proposal { submission_key: format!("team-{index}"), task: TaskKey { project: developer.project.clone(), task_id: proposal.task.task_id.clone() }, members, reason: "Own team".into() };
        let pending = store::propose(connection, developer, &request, live, 0).unwrap().result.unwrap();
        store::control(connection, &HostAction::Approve { request_id: format!("team-approval-{index}"), proposal_id: pending["proposal_id"].as_str().unwrap().into(),
            expected_revision: 1, mapping_hash: pending["mapping_hash"].as_str().unwrap().into() }, live, 1).unwrap();
    }
}

fn activate(connection: &mut Connection, sender: &VerifiedSession, proposal: &LinkProposal, live: &[VerifiedSession]) -> String {
    let pending = links::propose(connection, sender, proposal, live, 0).unwrap().result.unwrap();
    let approval = link_approval(connection, &pending, &format!("approve-{}", proposal.submission_key));
    store::control(connection, &approval, live, 1).unwrap().result.unwrap()["link_id"].as_str().unwrap().into()
}

fn note(link_id: &str, key: &str) -> teams::LinkSubmission {
    teams::LinkSubmission { link_id: link_id.into(), submission_key: key.into(), kind: MessageKind::Notice,
        body: "Coordinate work".into(), thread_id: None }
}

#[test]
fn chief_grants_cannot_acquire_link_authority() {
    if isolated("chief_grants_cannot_acquire_link_authority") { return; }
    use cockpit::chief::{ChiefHostAction, WorkerState, store as chief};
    let (mut connection, _, live, proposal) = proposal_fixture();
    let pending = links::propose(&mut connection, &live[0], &proposal, &live, 0).unwrap().result.unwrap();
    let approval = link_approval(&connection, &pending, "approve-after-chief");
    let workers: Vec<_> = live.iter().map(|session| WorkerState { session:session.clone(), role:None, ready:false, blocked_by:vec![], observed_at_ms:0 }).collect();
    chief::host(&mut connection, &ChiefHostAction::Enable { request_id:"enable".into(), session:live[1].clone() }, &workers, 1).unwrap().result.unwrap();
    assert!(links::propose(&mut connection, &live[0], &LinkProposal { submission_key:"new-request".into(), ..proposal }, &live, 2).is_err());
    assert!(store::control(&mut connection, &approval, &live, 3).is_err());
    assert_eq!(connection.query_row("SELECT count(*) FROM links", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn link_approval_neither_authorizes_chief_actions_nor_allows_overlapping_grants() {
    if isolated("link_approval_neither_authorizes_chief_actions_nor_allows_overlapping_grants") { return; }
    use cockpit::chief::{ChiefHostAction, ChiefAgentAction, WorkerState, store as chief};
    let (mut connection, _, live, proposal) = proposal_fixture();
    activate(&mut connection, &live[0], &proposal, &live);
    let workers: Vec<_> = live.iter().map(|session| WorkerState { session:session.clone(), role:None, ready:false, blocked_by:vec![], observed_at_ms:0 }).collect();
    assert_eq!(chief::agent(&mut connection, &live[0], &ChiefAgentAction::Context { after:0, limit:50 }, &workers, 1).unwrap().result.unwrap_err().code, ErrorCode::Unauthorized);
    assert!(chief::host(&mut connection, &ChiefHostAction::Enable { request_id:"overlap".into(), session:live[0].clone() }, &workers, 2).unwrap().result.is_err());
}

#[test]
fn link_delivery_blocks_chief_delivery_until_a_new_native_boundary() {
    if isolated("link_delivery_blocks_chief_delivery_until_a_new_native_boundary") { return; }
    assert_link_chief_inbox_exclusion(false);
}

#[test]
fn chief_delivery_blocks_link_delivery_until_a_new_native_boundary() {
    if isolated("chief_delivery_blocks_link_delivery_until_a_new_native_boundary") { return; }
    assert_link_chief_inbox_exclusion(true);
}

fn assert_link_chief_inbox_exclusion(chief_first: bool) {
    use cockpit::chief::{ChiefHostAction, ChiefAgentAction, WorkerState, store as chief};
    let (mut connection, _, mut live, proposal) = proposal_fixture();
    live[2].agent = "opencode".into();
    live[2].session_id = "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L".into();
    let link = activate(&mut connection, &live[0], &proposal, &live);
    links::accept(&mut connection, &live[1], &note(&link, "link-notice"), &live, 1000).unwrap();
    let workers: Vec<_> = live.iter().map(|session| WorkerState { session:session.clone(), role:Some(Role::Developer), ready:true, blocked_by:vec![], observed_at_ms:1000 }).collect();
    chief::host(&mut connection, &ChiefHostAction::Enable { request_id:"enable".into(), session:live[2].clone() }, &workers, 1000).unwrap().result.unwrap();
    chief::host(&mut connection, &ChiefHostAction::BindWorker { request_id:"bind".into(), session:live[0].clone(), role:Role::Developer, expected_epoch:None }, &workers, 1000).unwrap().result.unwrap();
    let mut brief = common::chief::brief();
    brief.work.task = proposal.task;
    let work = brief.work.clone();
    chief::host(&mut connection, &ChiefHostAction::ApproveWork { request_id:"work".into(), approval_hash:brief.approval_hash().unwrap(), brief, expected_revision:0 }, &workers, 1000).unwrap().result.unwrap();
    common::chief::admit(&mut connection, &workers, &live[2], 1000);
    chief::agent(&mut connection, &live[2], &ChiefAgentAction::Dispatch { submission_key:"assignment".into(), work, worker:live[0].session_id.clone() }, &workers, 1000).unwrap().result.unwrap();
    chief::observe(&mut connection, &workers, 1000).unwrap();
    let first = if chief_first { chief::claim(&mut connection, &live[0], "idle-1", 1000) }
        else { store::claim(&mut connection, &live[0], "idle-1", &live, 1000) }.unwrap().unwrap();
    let second = if chief_first { store::claim(&mut connection, &live[0], "idle-2", &live, 1000) }
        else { chief::claim(&mut connection, &live[0], "idle-2", 1000) }.unwrap();
    assert!(second.is_none());
    let received = teams::TransportOutcome::Received { native_id:"native-first".into() };
    if chief_first { chief::finish_for(&mut connection, &live[0], &first.id, &received, 1001).unwrap(); }
    else { store::finish_for(&mut connection, &live[0], &first.id, &received, 1001).unwrap(); }
    let reused = if chief_first { store::claim(&mut connection, &live[0], "idle-1", &live, 1002) }
        else { chief::claim(&mut connection, &live[0], "idle-1", 1002) }.unwrap();
    assert!(reused.is_none());
    let next = if chief_first { store::claim(&mut connection, &live[0], "idle-2", &live, 1002) }
        else { chief::claim(&mut connection, &live[0], "idle-2", 1002) }.unwrap();
    assert!(next.is_some());
}

#[test]
fn stale_recent_done_priority_save_cannot_pause_and_revive_a_new_approval() {
    if isolated("stale_recent_done_priority_save_cannot_pause_and_revive_a_new_approval") { return; }
    let (mut connection, mut current, live, proposal) = proposal_fixture();
    assert!(current.set_status(&proposal.task.task_id, "done", ""));
    let mut stale = todo::TodoList::load();
    assert!(current.set_status(&proposal.task.task_id, "todo", ""));
    assert_stale_save_preserves_open_work(&mut connection, &mut current, &live, &proposal, || stale.change_priority(&proposal.task.task_id, 1));
}

#[test]
fn stale_omission_edit_save_cannot_remove_newly_approved_work() {
    if isolated("stale_omission_edit_save_cannot_remove_newly_approved_work") { return; }
    let mut stale = todo::TodoList::load();
    let other = stale.add("other", "/untracked", "Other work");
    let (mut connection, mut current, live, proposal) = proposal_fixture();
    assert_stale_save_preserves_open_work(&mut connection, &mut current, &live, &proposal, || {
        assert!(stale.edit(&other, "Edited other work", "Unrelated notes"));
    });
}

fn assert_stale_save_preserves_open_work(connection: &mut Connection, current: &mut todo::TodoList, live: &[VerifiedSession], proposal: &LinkProposal, save: impl FnOnce()) {
    let link = activate(connection, &live[0], proposal, live);
    let pending = links::propose(connection, &live[2], &LinkProposal { submission_key:"pending-after-snapshot".into(), ..proposal.clone() }, live, 0).unwrap().result.unwrap();
    let approval = link_approval(connection, &pending, "late-approval");
    let queued = links::accept(connection, &live[0], &note(&link, "queued-after-snapshot"), live, 1000).unwrap().result.unwrap();
    let before = fs::read(cockpit::paths::todo()).unwrap();
    save();
    assert_eq!(fs::read(cockpit::paths::todo()).unwrap(), before, "a stale terminal/removal snapshot must be rejected before JSON publication");
    assert_eq!(todo::TodoList::load().status_of(&proposal.task.task_id), "todo");
    assert!(current.set_status(&proposal.task.task_id, "todo", ""));
    let dispatch = store::claim(connection, &live[1], "still-open", live, 2000).unwrap().unwrap();
    assert_eq!(json!(dispatch.message_id), queued["message_id"]);
    store::finish(connection, &dispatch.id, &teams::TransportOutcome::Received { native_id:"received".into() }, 2001).unwrap();
    // A real terminal transition still closes this exact approval and its pending peer request.
    let safe = links::accept(connection, &live[0], &note(&link, "safe-before-done"), live, 61_001).unwrap().result.unwrap();
    assert!(current.set_status(&proposal.task.task_id, "done", ""));
    assert!(current.set_status(&proposal.task.task_id, "todo", ""));
    assert_eq!(connection.query_row("SELECT state FROM links WHERE id=?1", [&link], |row| row.get::<_, String>(0)).unwrap(), "closed");
    assert_eq!(connection.query_row("SELECT status FROM link_proposals WHERE id=?1", [pending["proposal_id"].as_str().unwrap()], |row| row.get::<_, String>(0)).unwrap(), "closed");
    assert_eq!(connection.query_row("SELECT state FROM messages WHERE id=?1", [safe["message_id"].as_str().unwrap()], |row| row.get::<_, String>(0)).unwrap(), "cancelled");
    assert_eq!(store::control(connection, &approval, live, 62_000).unwrap_err().code, ErrorCode::Conflict);
    assert!(links::accept(connection, &live[0], &note(&link, "cannot-revive"), live, 62_001).is_err());
    assert!(store::claim(connection, &live[1], "after-reopen", live, 62_002).unwrap().is_none());
}

#[test]
fn link_context_is_bound_to_participant_identity_and_preserves_own_team() {
    if isolated("link_context_is_bound_to_participant_identity_and_preserves_own_team") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    own_teams(&mut connection, &live, &proposal);
    let team_before = store::context(&connection, &live[0]).unwrap();
    let link = activate(&mut connection, &live[0], &proposal, &live);
    let foreign = activate(&mut connection, &live[2], &LinkProposal { submission_key: "foreign".into(), ..proposal.clone() }, &live);
    let context = store::context(&connection, &live[0]).unwrap();
    assert_eq!(context["task"], team_before["task"]);
    assert_eq!(context["members"], team_before["members"]);
    assert_eq!(context["links"]["approved"].as_array().unwrap().len(), 1);
    let approved = &context["links"]["approved"][0];
    assert_eq!(approved["link_id"], link);
    assert_eq!(approved["task"], json!(proposal.task));
    assert_eq!(approved["purpose"], "Coordinate shared work");
    assert_eq!(approved["peer"], json!({"session_id":live[1].session_id,"agent":"claude","project":live[1].project}));
    assert_eq!(approved["status"], "active");
    assert!(!context.to_string().contains(&foreign));
    for field in ["runtime", "project", "agent"] {
        let mut changed = live[0].clone();
        match field { "runtime" => changed.runtime = "replacement".into(), "project" => changed.project = "path:/other".into(), _ => changed.agent = "codex".into() }
        let context = store::context(&connection, &changed).unwrap();
        assert_eq!(context["links"], json!({"approved":[],"proposals":[]}), "{field}");
    }
    store::control(&mut connection, &HostAction::LinkRevoke { request_id:"revoke".into(), link_id:link }, &live, 100).unwrap();
    let context = store::context(&connection, &live[0]).unwrap();
    assert_eq!(context["links"]["approved"][0]["status"], "closed");
    assert_eq!(context["links"]["approved"][0]["closure_reason"], "revoked");
}

#[test]
fn link_context_shows_only_requesters_own_status_receipts() {
    if isolated("link_context_shows_only_requesters_own_status_receipts") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    let pending = links::propose(&mut connection, &live[0], &proposal, &live, 0).unwrap().result.unwrap();
    links::propose(&mut connection, &live[2], &LinkProposal { submission_key:"other".into(), ..proposal.clone() }, &live, 0).unwrap();
    for status in ["pending", "rejected"] {
        if status == "rejected" {
            store::control(&mut connection, &HostAction::LinkReject { request_id:"reject".into(), proposal_id:pending["proposal_id"].as_str().unwrap().into(), expected_revision:1 }, &live, 1).unwrap();
        }
        let context = store::context(&connection, &live[0]).unwrap();
        assert_eq!(context["links"], json!({"approved":[],"proposals":[{"proposal_id":pending["proposal_id"],"revision":1,"status":status}]}));
        assert!(!context.to_string().contains(&live[1].session_id));
        assert_eq!(store::context(&connection, &live[1]).unwrap()["links"], json!({"approved":[],"proposals":[]}));
    }
}

#[test]
fn historical_context_is_byte_bounded_without_hiding_current_links_or_pending_requests() {
    if isolated("historical_context_is_byte_bounded_without_hiding_current_links_or_pending_requests") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    own_teams(&mut connection, &live, &proposal);
    let active = activate(&mut connection, &live[0], &proposal, &live);
    let pending = links::propose(&mut connection, &live[0], &LinkProposal { submission_key:"still-pending".into(), ..proposal.clone() }, &live, 0).unwrap().result.unwrap();
    // The current records are older than all the terminal history: newest-N must not hide them.
    let history = LinkProposal { peer_session_id:live[2].session_id.clone(), ..proposal.clone() };
    link_history::populate(&mut connection, &live, &history);
    link_history::fill_reply_boundary(&mut connection, &live, &history);
    let context = store::context(&connection, &live[0]).unwrap();
    let reply = teams::TeamReply { request_id:"context".into(), result:Ok(context.clone()) };
    let framed_reply = format!("{}\n", serde_json::to_string(&reply).unwrap());
    assert!(framed_reply.len() <= teams::client::AGENT_REPLY_MAX, "framed Context has {} bytes, exceeding the native transport cap", framed_reply.len());
    let approved = context["links"]["approved"].as_array().unwrap();
    let proposals = context["links"]["proposals"].as_array().unwrap();
    assert!(approved.iter().any(|link| link["link_id"] == active && link["status"] == "active" && link["purpose"] == proposal.purpose));
    assert!(proposals.contains(&pending));
    assert_eq!(context["members"].as_array().unwrap().len(), 2);
    assert_eq!(approved.len() as u64 + context["links"]["omitted_history"]["approved"].as_u64().unwrap(), 42);
    assert_eq!(proposals.len() as u64 + context["links"]["omitted_history"]["proposals"].as_u64().unwrap(), 83);
    assert_eq!(connection.query_row("SELECT count(*) FROM links WHERE state='closed'", [], |row| row.get::<_, i64>(0)).unwrap(), 40);
    assert_eq!(connection.query_row("SELECT count(*) FROM link_proposals WHERE status='rejected'", [], |row| row.get::<_, i64>(0)).unwrap(), 40);
    assert_eq!(store::context(&connection, &live[1]).unwrap()["links"]["approved"].as_array().unwrap().len(), 1);
}

#[test]
fn host_link_snapshot_preserves_current_requests_and_exact_closed_mappings_past_history_limit() {
    if isolated("host_link_snapshot_preserves_current_requests_and_exact_closed_mappings_past_history_limit") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    let active = activate(&mut connection, &live[0], &proposal, &live);
    let pending = links::propose(&mut connection,&live[0],&LinkProposal {submission_key:"pending".into(),..proposal.clone()},&live,0).unwrap().result.unwrap();
    link_history::populate(&mut connection,&live,&LinkProposal {peer_session_id:live[2].session_id.clone(),..proposal.clone()});
    let snapshot = store::snapshot(&connection,None,None,2).unwrap();
    let links = snapshot["links"].as_array().unwrap();
    assert!(links.iter().any(|link| link["link_id"] == active && link["state"] == "active"));
    assert!(snapshot["link_proposals"].as_array().unwrap().iter().any(|request| request["proposal_id"] == pending["proposal_id"] && request["status"] == "pending"));
    let closed: Vec<_> = links.iter().filter(|link| link["state"] == "closed").collect();
    assert_eq!(closed.len(),2);
    for link in closed {
        assert_eq!(link["closure_reason"],"revoked");
        assert_eq!(link["participants"],json!([live[0],live[2]]));
        assert_eq!(link["task"],json!(proposal.task));
        assert_eq!(link["revision"],1);
        assert_eq!(link["mapping_hash"].as_str().unwrap().len(),64);
    }
    let other = TaskKey {task_id:"unrelated".into(),..proposal.task};
    let scoped = store::snapshot(&connection,Some(&other),None,2).unwrap();
    assert_eq!(scoped["links"],json!([]));
    assert_eq!(scoped["link_proposals"],json!([]));
}

#[test]
fn approved_link_delivers_without_team_membership() {
    if isolated("approved_link_delivers_without_team_membership") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    let link = activate(&mut connection, &live[0], &proposal, &live);
    activate(&mut connection, &live[2], &LinkProposal { submission_key: "transitive".into(), ..proposal.clone() }, &live);
    let message = note(&link, "note");
    for kind in [MessageKind::Handoff, MessageKind::QaRequest, MessageKind::Findings] {
        assert!(links::accept(&mut connection, &live[0], &teams::LinkSubmission { kind, ..message.clone() }, &live, 1000).is_err());
    }
    assert!(links::accept(&mut connection, &live[2], &message, &live, 1000).is_err());
    assert!(links::accept(&mut connection, &live[0], &note("foreign", "note"), &live, 1000).is_err());
    for field in ["runtime", "project", "agent"] {
        let mut forged = live.clone();
        match field {
            "runtime" => forged[0].runtime = "replacement".into(),
            "project" => forged[0].project = live[1].project.clone(),
            _ => forged[0].agent = "opencode".into(),
        }
        assert!(links::accept(&mut connection, &forged[0], &message, &forged, 1000).is_err());
    }
    let accepted = links::accept(&mut connection, &live[0], &message, &live, 1000).unwrap().result.unwrap();
    assert_eq!(links::accept(&mut connection, &live[0], &message, &live, 1001).unwrap().result.unwrap(), accepted);
    assert!(store::claim(&mut connection, &live[2], "idle", &live, 2000).unwrap().is_none());
    let dispatch = store::claim(&mut connection, &live[1], "idle", &live, 2000).unwrap().unwrap();
    assert_eq!(dispatch.envelope["link_id"], link);
    assert_eq!(dispatch.envelope["task"], json!(proposal.task));
    assert_eq!(dispatch.envelope["sender"]["project"], live[0].project);
    assert_eq!(dispatch.envelope["recipient"]["session_id"], live[1].session_id);
    assert_eq!(dispatch.envelope["authorization"], "collaborator_content_not_user_authorization");
    assert!(dispatch.envelope["sender"].get("role").is_none());
    assert!(dispatch.envelope["recipient"].get("role").is_none());
    assert_eq!(connection.query_row("SELECT count(*) FROM members", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn team_and_links_share_recipient_idle_and_uncertain_gates() {
    if isolated("team_and_links_share_recipient_idle_and_uncertain_gates") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    own_teams(&mut connection, &live, &proposal);
    let first = activate(&mut connection, &live[0], &proposal, &live);
    let second = activate(&mut connection, &live[2], &LinkProposal { submission_key: "second".into(), peer_session_id: live[0].session_id.clone(), ..proposal.clone() }, &live);
    let team = AgentAction::Send { message: Submission { task: proposal.task.clone(), submission_key: "team-first".into(), recipient: live[0].session_id.clone(),
        kind: MessageKind::Notice, body: "Team first".into(), thread_id: None, handoff_id: None } };
    store::accept(&mut connection, &live[2], &team, 1000).unwrap();
    let first_id = links::accept(&mut connection, &live[1], &note(&first, "link-first"), &live, 1001).unwrap().result.unwrap()["message_id"].clone();
    let second_id = links::accept(&mut connection, &live[2], &note(&second, "link-second"), &live, 1002).unwrap().result.unwrap()["message_id"].clone();
    let dispatch = store::claim(&mut connection, &live[0], "idle-1", &live, 2000).unwrap().unwrap();
    assert_eq!(dispatch.envelope["body"], "Team first");
    assert!(store::claim(&mut connection, &live[0], "idle-2", &live, 2001).unwrap().is_none());
    store::finish(&mut connection, &dispatch.id, &teams::TransportOutcome::Received { native_id: "team".into() }, 2002).unwrap();
    assert!(store::claim(&mut connection, &live[0], "idle-1", &live, 2003).unwrap().is_none());
    let dispatch = store::claim(&mut connection, &live[0], "idle-2", &live, 2004).unwrap().unwrap();
    assert_eq!(json!(dispatch.message_id), first_id);
    store::expire(&mut connection, 2004 + store::RECEIPT_TIMEOUT_MS).unwrap();
    assert!(store::claim(&mut connection, &live[0], "idle-3", &live, 200_000).unwrap().is_none());
    assert!(store::finish(&mut connection, &dispatch.id, &teams::TransportOutcome::NotSubmitted { reason: "late".into() }, 200_001).is_err());
    store::finish(&mut connection, &dispatch.id, &teams::TransportOutcome::Received { native_id: "link".into() }, 200_002).unwrap();
    assert_eq!(json!(store::claim(&mut connection, &live[0], "idle-3", &live, 200_003).unwrap().unwrap().message_id), second_id);
}

#[test]
fn link_threads_are_scope_bound_and_retry_does_not_consume_rounds() {
    if isolated("link_threads_are_scope_bound_and_retry_does_not_consume_rounds") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    own_teams(&mut connection, &live, &proposal);
    let link = activate(&mut connection, &live[0], &proposal, &live);
    let foreign = activate(&mut connection, &live[2], &LinkProposal { submission_key: "foreign".into(), ..proposal.clone() }, &live);
    let mut thread_id = None;
    for round in 1..=3 {
        let question = teams::LinkSubmission { kind: MessageKind::Clarification, thread_id: thread_id.clone(), ..note(&link, &format!("question-{round}")) };
        let accepted = links::accept(&mut connection, &live[0], &question, &live, round * 60_001).unwrap().result.unwrap();
        assert_eq!(links::accept(&mut connection, &live[0], &question, &live, round * 60_001).unwrap().result.unwrap(), accepted);
        assert_eq!(accepted["round"], round);
        thread_id = Some(accepted["thread_id"].as_str().unwrap().into());
        let answer = teams::LinkSubmission { kind: MessageKind::Reply, thread_id: thread_id.clone(), ..note(&link, &format!("answer-{round}")) };
        assert!(links::accept(&mut connection, &live[0], &answer, &live, round * 60_001).is_err());
        assert!(links::accept(&mut connection, &live[1], &teams::LinkSubmission { link_id: foreign.clone(), ..answer.clone() }, &live, round * 60_001).is_err());
        links::accept(&mut connection, &live[1], &answer, &live, round * 60_001).unwrap();
        assert!(links::assess(&mut connection, &live[1], &link, thread_id.as_deref().unwrap(), false).is_err());
        assert!(links::assess(&mut connection, &live[0], &foreign, thread_id.as_deref().unwrap(), false).is_err());
        let assessed = links::assess(&mut connection, &live[0], &link, thread_id.as_deref().unwrap(), false).unwrap().result.unwrap();
        assert_eq!(assessed["status"], if round == 3 { "escalated" } else { "awaiting_followup" });
    }
    assert_eq!(connection.query_row("SELECT count(*) FROM escalations WHERE status='open'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
}

#[test]
fn link_retry_closure_recovery_and_identity_checks_fail_closed() {
    if isolated("link_retry_closure_recovery_and_identity_checks_fail_closed") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    let link = activate(&mut connection, &live[0], &proposal, &live);
    links::accept(&mut connection, &live[0], &note(&link, "note"), &live, 1000).unwrap();
    assert!(store::claim(&mut connection, &live[1], "idle", &live[1..], 2000).unwrap().is_none());
    let original = fs::read(cockpit::paths::todo()).unwrap();
    fs::write(cockpit::paths::todo(), "{broken").unwrap();
    assert!(store::claim(&mut connection, &live[1], "idle", &live, 2000).unwrap().is_none());
    assert!(links::accept(&mut connection, &live[0], &note(&link, "blocked"), &live, 2000).is_err());
    fs::write(cockpit::paths::todo(), original).unwrap();
    let mut now = 2000;
    for delay in [2000, 10_000, 30_000] {
        let dispatch = store::claim(&mut connection, &live[1], "idle", &live, now).unwrap().unwrap();
        store::finish(&mut connection, &dispatch.id, &teams::TransportOutcome::NotSubmitted { reason: "safe".into() }, now).unwrap();
        assert!(store::claim(&mut connection, &live[1], "idle", &live, now + delay - 1).unwrap().is_none());
        now += delay;
    }
    let dispatch = store::claim(&mut connection, &live[1], "idle", &live, now).unwrap().unwrap();
    store::finish(&mut connection, &dispatch.id, &teams::TransportOutcome::NotSubmitted { reason: "fourth".into() }, now).unwrap();
    assert!(store::claim(&mut connection, &live[1], "next", &live, now + 100_000).unwrap().is_none());
    links::accept(&mut connection, &live[0], &note(&link, "uncertain"), &live, now + 100_000).unwrap();
    let dispatch = store::claim(&mut connection, &live[1], "next", &live, now + 100_000).unwrap().unwrap();
    links::invalidate_work(&mut connection, &proposal.task, "done").unwrap();
    store::recover(&mut connection, &live, now + 100_001).unwrap();
    assert!(store::finish(&mut connection, &dispatch.id, &teams::TransportOutcome::NotSubmitted { reason: "late".into() }, now + 100_002).is_err());
    store::finish(&mut connection, &dispatch.id, &teams::TransportOutcome::Received { native_id: "late".into() }, now + 100_003).unwrap();
    assert!(store::claim(&mut connection, &live[1], "later", &live, now + 200_000).unwrap().is_none());
}

#[test]
fn link_messages_share_budget_queue_and_submission_keys_with_teams() {
    if isolated("link_messages_share_budget_queue_and_submission_keys_with_teams") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    own_teams(&mut connection, &live, &proposal);
    let first = activate(&mut connection, &live[0], &proposal, &live);
    let second = activate(&mut connection, &live[2], &LinkProposal { submission_key: "second".into(), ..proposal.clone() }, &live);
    let team = AgentAction::Send { message: Submission { task: proposal.task.clone(), submission_key: "shared-key".into(), recipient: live[2].session_id.clone(),
        kind: MessageKind::Notice, body: "Team".into(), thread_id: None, handoff_id: None } };
    store::accept(&mut connection, &live[0], &team, 1000).unwrap();
    assert_eq!(links::accept(&mut connection, &live[0], &note(&first, "shared-key"), &live, 1000).unwrap_err().code, ErrorCode::Conflict);
    for index in 0..7 {
        let message = note(&first, &format!("budget-{index}"));
        links::accept(&mut connection, &live[0], &message, &live, 1000).unwrap();
        links::accept(&mut connection, &live[0], &message, &live, 1000).unwrap();
    }
    assert_eq!(links::accept(&mut connection, &live[0], &note(&first, "overflow"), &live, 1000).unwrap_err().code, ErrorCode::Limit);
    let AgentAction::Send { mut message } = team else { unreachable!() };
    message.submission_key = "budget-0".into();
    assert_eq!(store::accept(&mut connection, &live[0], &AgentAction::Send { message }, 1000).unwrap_err().code, ErrorCode::Conflict);
    for index in 8..100 {
        let (link, sender) = if index % 2 == 0 { (&first, &live[0]) } else { (&second, &live[2]) };
        links::accept(&mut connection, sender, &note(link, &format!("pending-{index}")), &live, index * 60_001).unwrap();
    }
    assert_eq!(links::accept(&mut connection, &live[2], &note(&second, "full"), &live, 7_000_000).unwrap_err().code, ErrorCode::Limit);
    assert_eq!(connection.query_row("SELECT count(*) FROM messages", [], |row| row.get::<_, i64>(0)).unwrap(), 100);
}

#[test]
fn recovery_cancels_replaced_link_identities_but_preserves_team_delivery() {
    if isolated("recovery_cancels_replaced_link_identities_but_preserves_team_delivery") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    own_teams(&mut connection, &live, &proposal);
    let link = activate(&mut connection, &live[0], &proposal, &live);
    links::accept(&mut connection, &live[1], &note(&link, "link"), &live, 1000).unwrap();
    let team = AgentAction::Send { message: Submission { task: proposal.task.clone(), submission_key: "team".into(), recipient: live[0].session_id.clone(),
        kind: MessageKind::Notice, body: "Healthy team".into(), thread_id: None, handoff_id: None } };
    store::accept(&mut connection, &live[2], &team, 1001).unwrap();
    store::recover(&mut connection, &[live[0].clone(), live[2].clone()], 2000).unwrap();
    assert_eq!(connection.query_row("SELECT state FROM messages WHERE scope_kind='link'", [], |row| row.get::<_, String>(0)).unwrap(), "queued");
    let mut changed = live.clone();
    changed[1].runtime = "replacement".into();
    assert!(links::accept(&mut connection, &changed[1], &note(&link, "new"), &changed, 2000).is_err());
    assert!(links::accept(&mut connection, &live[0], &note(&link, "new"), &changed, 2000).is_err());
    store::recover(&mut connection, &changed, 2001).unwrap();
    assert_eq!(connection.query_row("SELECT state FROM messages WHERE scope_kind='link'", [], |row| row.get::<_, String>(0)).unwrap(), "cancelled");
    assert_eq!(store::claim(&mut connection, &live[0], "idle", &changed, 2002).unwrap().unwrap().envelope["body"], "Healthy team");
}

#[test]
fn closure_and_claim_are_ordered_across_connections() {
    if isolated("closure_and_claim_are_ordered_across_connections") { return; }
    use std::sync::{Arc, Barrier};
    let (mut connection, mut todos, live, proposal) = proposal_fixture();
    for close_first in [true, false] {
        let proposal = LinkProposal { submission_key: format!("order-{close_first}"), ..proposal.clone() };
        let link = activate(&mut connection, &live[0], &proposal, &live);
        links::accept(&mut connection, &live[0], &note(&link, &format!("note-{close_first}")), &live, 1000).unwrap();
        let boundary = Arc::new(Barrier::new(2));
        let peer_boundary = boundary.clone();
        let task = proposal.task.clone();
        let recipient = live[1].clone();
        let current = live.clone();
        let other = store::open(&cockpit::paths::team_database()).unwrap();
        let worker = std::thread::spawn(move || {
            let mut other = other;
            if close_first {
                links::invalidate_work(&mut other, &task, "done").unwrap();
                peer_boundary.wait();
                None
            } else {
                let dispatch = store::claim(&mut other, &recipient, "idle", &current, 2000).unwrap();
                peer_boundary.wait();
                dispatch
            }
        });
        boundary.wait();
        if close_first {
            assert!(store::claim(&mut connection, &live[1], "idle", &live, 2001).unwrap().is_none());
            worker.join().unwrap();
        } else {
            links::invalidate_work(&mut connection, &proposal.task, "done").unwrap();
            let dispatch = worker.join().unwrap().unwrap();
            assert_eq!(connection.query_row("SELECT state FROM messages WHERE id=?1", [&dispatch.message_id], |row| row.get::<_, String>(0)).unwrap(), "submitting");
            store::finish(&mut connection, &dispatch.id, &teams::TransportOutcome::NotSubmitted { reason: "safe failure after closure".into() }, 2002).unwrap();
            assert_eq!(connection.query_row("SELECT state FROM messages WHERE id=?1", [&dispatch.message_id], |row| row.get::<_, String>(0)).unwrap(), "cancelled");
            assert!(store::claim(&mut connection, &live[1], "new-idle", &live, 100_000).unwrap().is_none());
        }
        assert!(todos.set_status(&proposal.task.task_id, "todo", ""));
    }
}

#[test]
fn delivery_migration_preserves_receipts_and_reconciles_multiple_inflight() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("protected/teams.sqlite");
    let connection = store::open(&path).unwrap();
    connection.execute_batch("DROP TABLE delivery_recipients; ALTER TABLE threads DROP COLUMN link_id; ALTER TABLE threads DROP COLUMN scope_kind;
        DROP INDEX one_inflight_delivery; CREATE UNIQUE INDEX one_inflight_delivery ON messages(recipient,recipient_epoch) WHERE state='submitting';
        PRAGMA user_version=11;
        INSERT INTO teams VALUES ('task','project','1234abcd',1);
        INSERT INTO members VALUES ('task','recipient','developer','claude','runtime','epoch','old-idle');").unwrap();
    for (id, state) in [("one", "submitting"), ("two", "submitting"), ("received", "received"), ("queued", "queued")] {
        connection.execute("INSERT INTO messages (id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime)
            VALUES (?1,'task','sender','recipient','sender-epoch',?1,?1,'fingerprint','{}',?2,0,0,'runtime')", (id, state)).unwrap();
        if state != "queued" {
            connection.execute("INSERT INTO attempts VALUES (?1,?1,1,?2,?3,9,1)", (id, state, if state == "received" { Some("native-receipt") } else { None })).unwrap();
        }
    }
    drop(connection);
    let connection = store::open(&path).unwrap();
    for table in ["messages", "attempts"] {
        assert_eq!(connection.query_row(&format!("SELECT count(*) FROM {table} WHERE state='uncertain'"), [], |row| row.get::<_, i64>(0)).unwrap(), 2);
    }
    assert_eq!(connection.query_row("SELECT native_id FROM attempts WHERE id='received'", [], |row| row.get::<_, String>(0)).unwrap(), "native-receipt");
    assert_eq!(connection.query_row("SELECT state FROM messages WHERE id='queued'", [], |row| row.get::<_, String>(0)).unwrap(), "queued");
    assert_eq!(connection.query_row("SELECT last_idle_turn FROM delivery_recipients WHERE session_id='recipient' AND runtime='runtime'", [], |row| row.get::<_, String>(0)).unwrap(), "old-idle");
    connection.execute("UPDATE messages SET state='submitting' WHERE id='one'", []).unwrap();
    assert!(connection.execute("UPDATE messages SET state='submitting' WHERE id='two'", []).is_err());
}

#[test]
fn link_only_queue_never_authorizes_team_sleep_wake_or_runtime_rebinding() {
    if isolated("link_only_queue_never_authorizes_team_sleep_wake_or_runtime_rebinding") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    own_teams(&mut connection, &live, &proposal);
    let link = activate(&mut connection, &live[0], &proposal, &live);
    let checkout = live[0].project.trim_start_matches("path:");
    let token = store::sleep::prepare(&mut connection, &live[0], checkout, 1000).unwrap().unwrap();
    links::accept(&mut connection, &live[1], &note(&link, "sleeping"), &live, 1001).unwrap();
    assert!(store::sleep::pending_wakes(&connection, 1002).unwrap().is_empty());
    assert!(!store::sleep::reserve_wake(&mut connection, &live[0].session_id, &token, 1002).unwrap());
    store::sleep::begin_wake(&mut connection, &live[0].session_id, &token, 1003).unwrap();
    let mut replacement = live[0].clone();
    replacement.runtime = "replacement".into();
    assert!(store::sleep::bind(&mut connection, &token, &replacement, checkout, 1004).unwrap());
    assert_eq!(connection.query_row("SELECT recipient_runtime FROM messages WHERE scope_kind='link'", [], |row| row.get::<_, String>(0)).unwrap(), live[0].runtime);
}

#[test]
fn link_only_escalation_is_visible_in_host_history() {
    if isolated("link_only_escalation_is_visible_in_host_history") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    let link = activate(&mut connection, &live[0], &proposal, &live);
    let mut thread_id = None;
    for round in 1..=3 {
        let question = teams::LinkSubmission { kind: MessageKind::Clarification, thread_id: thread_id.clone(), ..note(&link, &format!("question-{round}")) };
        let accepted = links::accept(&mut connection, &live[0], &question, &live, round * 60_001).unwrap().result.unwrap();
        thread_id = Some(accepted["thread_id"].as_str().unwrap().into());
        let answer = teams::LinkSubmission { kind: MessageKind::Reply, thread_id: thread_id.clone(), ..note(&link, &format!("answer-{round}")) };
        links::accept(&mut connection, &live[1], &answer, &live, round * 60_001).unwrap();
        links::assess(&mut connection, &live[0], &link, thread_id.as_deref().unwrap(), false).unwrap();
    }
    let history = store::snapshot(&connection, None, None, 100).unwrap();
    assert_eq!(history["escalations"].as_array().unwrap().len(), 1);
    assert_eq!(history["escalations"][0]["project"], proposal.task.project);
    assert_eq!(history["escalations"][0]["link_id"], link);
}

#[test]
fn claim_reads_authoritative_todo_only_after_waiting_for_the_writer() {
    if isolated("claim_reads_authoritative_todo_only_after_waiting_for_the_writer") { return; }
    use std::sync::atomic::{AtomicBool, Ordering};
    static WAITING: AtomicBool = AtomicBool::new(false);
    let (mut connection, _, live, proposal) = proposal_fixture();
    let link = activate(&mut connection, &live[0], &proposal, &live);
    links::accept(&mut connection, &live[0], &note(&link, "note"), &live, 1000).unwrap();
    let mut other = store::open(&cockpit::paths::team_database()).unwrap();
    other.busy_handler(Some(|_| { WAITING.store(true, Ordering::SeqCst); std::thread::sleep(std::time::Duration::from_millis(1)); true })).unwrap();
    let writer = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    let reader = std::thread::spawn(move || store::claim(&mut other, &live[1], "idle", &live, 2000));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !WAITING.load(Ordering::SeqCst) && !reader.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let waited = WAITING.load(Ordering::SeqCst);
    fs::write(cockpit::paths::todo(), "{broken").unwrap();
    writer.commit().unwrap();
    assert!(reader.join().unwrap().unwrap().is_none());
    assert!(waited, "claim bypassed the authoritative lifecycle writer");
    assert_eq!(connection.query_row("SELECT count(*) FROM attempts", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    assert!(!connection.query_row("SELECT last_error FROM messages", [], |row| row.get::<_, String>(0)).unwrap().is_empty());
}

#[test]
fn delivery_migration_failure_rolls_back_receipts_gates_and_schema() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("protected/teams.sqlite");
    let connection = store::open(&path).unwrap();
    connection.execute_batch("PRAGMA user_version=11; DROP TABLE delivery_recipients;
        ALTER TABLE threads DROP COLUMN scope_kind;
        DROP INDEX one_inflight_delivery; CREATE UNIQUE INDEX one_inflight_delivery ON messages(recipient,recipient_epoch) WHERE state='submitting';").unwrap();
    // The last ALTER in migration will collide; earlier DDL and reconciliation must roll back.
    for id in ["one", "two"] {
        connection.execute("INSERT INTO messages (id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime)
            VALUES (?1,'task','sender','recipient','sender-epoch',?1,?1,'fingerprint','{}','submitting',0,0,'runtime')", [id]).unwrap();
        connection.execute("INSERT INTO attempts VALUES (?1,?1,1,'submitting',NULL,NULL,1)", [id]).unwrap();
    }
    let before = team_state(&connection);
    drop(connection);
    assert!(store::open(&path).is_err());
    let connection = Connection::open(path).unwrap();
    assert_eq!(team_state(&connection), before);
    assert_eq!(connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 11);
    assert_eq!(connection.query_row("SELECT count(*) FROM sqlite_master WHERE name='delivery_recipients'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(connection.query_row("SELECT count(*) FROM pragma_table_info('threads') WHERE name='scope_kind'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn unrelated_sender_cannot_probe_link_work_errors() {
    if isolated("unrelated_sender_cannot_probe_link_work_errors") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    let link = activate(&mut connection, &live[0], &proposal, &live);
    fs::write(cockpit::paths::todo(), "{broken").unwrap();
    let unknown = links::accept(&mut connection, &live[2], &note("unknown", "probe"), &live, 1000).unwrap_err();
    let known = links::accept(&mut connection, &live[2], &note(&link, "probe"), &live, 1000).unwrap_err();
    assert_eq!(serde_json::to_value(known).unwrap(), serde_json::to_value(&unknown).unwrap());
    let known = links::assess(&mut connection, &live[2], &link, "probe", true).unwrap_err();
    assert_eq!(serde_json::to_value(known).unwrap(), serde_json::to_value(unknown).unwrap());
}

#[test]
fn cross_project_developers_require_exact_host_approval() {
    if isolated("cross_project_developers_require_exact_host_approval") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    own_teams(&mut connection, &live, &proposal);
    let before = store::snapshot(&connection, None, None, 100).unwrap();
    let todo_before = fs::read(cockpit::paths::todo()).unwrap();
    let pending = links::propose(&mut connection, &live[0], &proposal, &live, 1000).unwrap().result.unwrap();
    assert_eq!(pending["status"], "pending");
    assert!(!pending.to_string().contains(&live[1].session_id));
    assert!(!pending.to_string().contains(&live[1].project));
    let message = AgentAction::Send { message: Submission { task: proposal.task.clone(), submission_key: "premature".into(), recipient: live[1].session_id.clone(),
        kind: MessageKind::Notice, body: "Ready".into(), thread_id: None, handoff_id: None } };
    assert!(store::accept(&mut connection, &live[0], &message, 1100).is_err());
    assert_eq!(connection.query_row("SELECT count(*) FROM links", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    let approval = link_approval(&connection, &pending, "exact");
    let approved = store::control(&mut connection, &approval, &live, 2000).unwrap().result.unwrap();
    assert_eq!(approved["status"], "active");
    assert_eq!(store::control(&mut connection, &approval, &[], 2100).unwrap().result.unwrap(), approved);
    assert!(store::accept(&mut connection, &live[0], &message, 2200).is_err());
    let swapped = LinkProposal { submission_key: "reverse".into(), peer_session_id: live[0].session_id.clone(), ..proposal };
    let pending = links::propose(&mut connection, &live[1], &swapped, &live, 3000).unwrap().result.unwrap();
    let approval = link_approval(&connection, &pending, "duplicate-pair");
    assert!(store::control(&mut connection, &approval, &live, 4000).is_err());
    let after = store::snapshot(&connection, None, None, 100).unwrap();
    for (field, value) in before.as_object().unwrap() {
        if matches!(field.as_str(), "links" | "link_proposals") { continue }
        assert_eq!(&after[field], value, "link approval changed team field {field}");
    }
    assert_eq!(after["links"].as_array().unwrap().len(), 1);
    assert_eq!(after["links"][0]["state"], "active");
    assert_eq!(after["links"][0]["link_id"], approved["link_id"]);
    assert_eq!(after["link_proposals"].as_array().unwrap().len(), 2);
    assert_eq!(fs::read(cockpit::paths::todo()).unwrap(), todo_before);
}

#[test]
fn link_approval_revalidates_every_immutable_fact() {
    if isolated("link_approval_revalidates_every_immutable_fact") { return; }
    let (mut connection, mut todos, live, proposal) = proposal_fixture();
    for (index, field) in ["runtime", "requester_runtime", "project", "peer", "todo", "purpose", "revision", "hash", "unavailable", "generation"].into_iter().enumerate() {
        let mut proposal = proposal.clone();
        proposal.submission_key = format!("change-{field}");
        let pending = links::propose(&mut connection, &live[0], &proposal, &live, 1000 + index as i64 * 60_001).unwrap().result.unwrap();
        let mut approval = serde_json::to_value(link_approval(&connection, &pending, field)).unwrap();
        let mut changed = live.clone();
        match field {
            "runtime" => changed[1].runtime = "replacement".into(),
            "requester_runtime" => changed[0].runtime = "replacement".into(),
            "project" => changed[1].project = changed[0].project.clone(),
            "unavailable" => { changed.remove(1); },
            "revision" => approval["expected_revision"] = json!(2),
            "hash" => approval["mapping_hash"] = json!("forged"),
            "generation" => { assert!(todos.set_status(&proposal.task.task_id, "done", "")); assert!(todos.set_status(&proposal.task.task_id, "todo", "")); },
            _ => {
                let mut replacement = proposal.clone();
                if field == "peer" { replacement.peer_session_id = live[2].session_id.clone(); }
                if field == "purpose" { replacement.purpose = "Different purpose".into(); }
                if field == "todo" { replacement.task.task_id = todos.add("first", live[0].project.trim_start_matches("path:"), "Different work"); }
                let edited = store::control(&mut connection, &host_action(json!({"action":"link_edit","request_id":format!("edit-{field}"),
                    "proposal_id":pending["proposal_id"],"expected_revision":1,"replacement":replacement})), &live, 1100).unwrap().result.unwrap();
                assert_eq!(edited["revision"], 2);
                assert_ne!(edited["mapping_hash"], approval["mapping_hash"]);
            },
        }
        let stale_approval = store::control(&mut connection, &host_action(approval), &changed, 2000);
        assert!(stale_approval.is_err(), "accepted changed {field}");
        // Free the pending capacity without erasing history between cases.
        connection.execute("UPDATE link_proposals SET status='rejected' WHERE status='pending'", []).unwrap();
    }
}

#[test]
fn invalid_link_peers_are_indistinguishable_and_receipts_are_private() {
    if isolated("invalid_link_peers_are_indistinguishable_and_receipts_are_private") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    let mut errors = Vec::new();
    for invalid in ["unknown", "unavailable", "unrelated", "self", "unmanaged", "empty-runtime", "duplicate"] {
        let mut proposal = proposal.clone();
        let mut changed = live.clone();
        match invalid {
            "unknown" => proposal.peer_session_id = "dddddddd-0000-4000-8000-000000000000".into(),
            "unavailable" => { changed.remove(1); },
            "unrelated" => proposal.task.project = "path:/unrelated".into(),
            "self" => proposal.peer_session_id = live[0].session_id.clone(),
            "unmanaged" => changed[1].agent = "unmanaged".into(),
            "empty-runtime" => changed[1].runtime.clear(),
            "duplicate" => changed.push(live[1].clone()),
            _ => unreachable!(),
        }
        errors.push(serde_json::to_value(links::propose(&mut connection, &live[0], &proposal, &changed, 1000).unwrap_err()).unwrap());
    }
    assert!(errors.iter().all(|error| error == &errors[0]));
    let pending = links::propose(&mut connection, &live[0], &proposal, &live, 1000).unwrap().result.unwrap();
    store::control(&mut connection, &host_action(json!({"action":"link_reject","request_id":"reject","proposal_id":pending["proposal_id"],"expected_revision":1})), &live, 2000).unwrap();
    let rejected = links::propose(&mut connection, &live[0], &proposal, &live, 3000).unwrap().result.unwrap();
    assert_eq!(rejected, json!({"proposal_id":pending["proposal_id"],"revision":1,"status":"rejected"}));
    let approval = link_approval(&connection, &pending, "rejected-approval");
    assert!(store::control(&mut connection, &approval, &live, 4000).is_err());
}

#[test]
fn link_proposal_limits_validate_keys_and_count_pending_requests() {
    if isolated("link_proposal_limits_validate_keys_and_count_pending_requests") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    for key in ["".into(), "a".repeat(129), "not valid".into(), "é".into()] {
        assert!(links::propose(&mut connection, &live[0], &LinkProposal { submission_key: key, ..proposal.clone() }, &live, 1000).is_err());
    }
    for purpose in ["".into(), " ".into(), "a".repeat(1001), "two\nlines".into(), "control\u{1b}".into()] {
        assert!(links::propose(&mut connection, &live[0], &LinkProposal { purpose, ..proposal.clone() }, &live, 1000).is_err());
    }
    for index in 0..10 {
        let proposal = LinkProposal { submission_key: format!("{}-{index}", "a".repeat(126)), purpose: "é".repeat(1000), ..proposal.clone() };
        links::propose(&mut connection, &live[0], &proposal, &live, index * 60_001).unwrap();
    }
    assert_eq!(links::propose(&mut connection, &live[0], &proposal, &live, 700_000).unwrap_err().code, ErrorCode::Limit);
}

#[test]
fn link_and_team_traffic_share_one_sender_budget_and_replays_are_free() {
    if isolated("link_and_team_traffic_share_one_sender_budget_and_replays_are_free") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    own_teams(&mut connection, &live, &proposal);
    let message = AgentAction::Send { message: Submission { task: proposal.task.clone(), submission_key: "team-message".into(), recipient: live[2].session_id.clone(),
        kind: MessageKind::Notice, body: "Ready".into(), thread_id: None, handoff_id: None } };
    store::accept(&mut connection, &live[0], &message, 1000).unwrap();
    for index in 0..8 {
        let proposal = LinkProposal { submission_key: format!("link-{index}"), ..proposal.clone() };
        let receipt = links::propose(&mut connection, &live[0], &proposal, &live, 1000).unwrap().result.unwrap();
        assert_eq!(links::propose(&mut connection, &live[0], &proposal, &live, 1000).unwrap().result.unwrap(), receipt);
    }
    assert_eq!(links::propose(&mut connection, &live[0], &proposal, &live, 1000).unwrap_err().code, ErrorCode::Limit);
    let team = Proposal { submission_key: "extra-team".into(), task: proposal.task.clone(), members: vec![Member { session_id: live[0].session_id.clone(), role: Role::Developer }], reason: "Own work".into() };
    assert_eq!(store::propose(&mut connection, &live[0], &team, &live, 1000).unwrap_err().code, ErrorCode::Limit);
    let recruitment = teams::Recruitment { submission_key: "extra-recruit".into(), task: proposal.task.clone(), role: Role::Developer,
        recruits: vec![teams::Recruit { role: Role::Qa, brief: "Review this work".into() }], reason: "Review".into() };
    assert_eq!(store::recruit(&mut connection, &live[0], &recruitment, Some("claude"), &live, &Default::default(), 1000).unwrap_err().code, ErrorCode::Limit);
    let AgentAction::Send { mut message } = message else { unreachable!() };
    message.submission_key = "extra-message".into();
    assert_eq!(store::accept(&mut connection, &live[0], &AgentAction::Send { message }, 1000).unwrap_err().code, ErrorCode::Limit);
    assert_eq!(links::propose(&mut connection, &live[0], &LinkProposal { submission_key: "link-0".into(), purpose: "Changed".into(), ..proposal.clone() }, &live, 1000).unwrap_err().code, ErrorCode::Conflict);
    links::propose(&mut connection, &live[0], &proposal, &live, 61_001).unwrap();
}

#[test]
fn host_link_controls_are_idempotent_and_revocation_cancels_only_safe_deliveries() {
    if isolated("host_link_controls_are_idempotent_and_revocation_cancels_only_safe_deliveries") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    let originate = host_action(json!({"action":"link_propose","request_id":"host-create","requester_session_id":live[0].session_id,"proposal":proposal}));
    let pending = store::control(&mut connection, &originate, &live, 1000).unwrap().result.unwrap();
    assert_eq!(store::control(&mut connection, &originate, &[], 1001).unwrap().result.unwrap(), pending);
    let conflicting = host_action(json!({"action":"link_reject","request_id":"host-create","proposal_id":pending["proposal_id"],"expected_revision":1}));
    assert_eq!(store::control(&mut connection, &conflicting, &live, 1002).unwrap_err().code, ErrorCode::Conflict);
    let approval = link_approval(&connection, &pending, "approve");
    let approved = store::control(&mut connection, &approval, &live, 2000).unwrap().result.unwrap();
    for state in ["queued", "ready", "submitting", "uncertain", "received"] {
        connection.execute("INSERT INTO messages (id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime,scope_kind,link_id)
            VALUES (?1,'task','sender','recipient','a','b',?1,'fingerprint','{}',?1,0,0,'runtime','link',?2)", (state, approved["link_id"].as_str().unwrap())).unwrap();
    }
    let revoke = host_action(json!({"action":"link_revoke","request_id":"revoke","link_id":approved["link_id"]}));
    store::control(&mut connection, &revoke, &[], 3000).unwrap();
    store::control(&mut connection, &revoke, &[], 4000).unwrap();
    for (initial, expected) in [("queued", "cancelled"), ("ready", "cancelled"), ("submitting", "submitting"), ("uncertain", "uncertain"), ("received", "received")] {
        assert_eq!(connection.query_row("SELECT state FROM messages WHERE id=?1", [initial], |row| row.get::<_, String>(0)).unwrap(), expected);
    }
    let approval = link_approval(&connection, &pending, "restore");
    assert!(store::control(&mut connection, &approval, &live, 5000).is_err());
    assert_eq!(connection.query_row("SELECT closure_reason FROM links", [], |row| row.get::<_, String>(0)).unwrap(), "revoked");
}

#[test]
fn link_approval_caps_both_participants_at_ten_active_links() {
    if isolated("link_approval_caps_both_participants_at_ten_active_links") { return; }
    let (mut connection, mut todos, live, proposal) = proposal_fixture();
    for index in 0..10 {
        let task_id = todos.add("first", live[0].project.trim_start_matches("path:"), &format!("Joint work {index}"));
        let proposal = LinkProposal { submission_key: format!("work-{index}"), task: TaskKey { task_id, ..proposal.task.clone() }, ..proposal.clone() };
        let pending = links::propose(&mut connection, &live[0], &proposal, &live, index * 60_001).unwrap().result.unwrap();
        let approval = link_approval(&connection, &pending, &format!("approval-{index}"));
        store::control(&mut connection, &approval, &live, index * 60_001 + 1).unwrap();
    }
    for (index, peer) in [0, 1].into_iter().enumerate() {
        let proposal = LinkProposal { submission_key: format!("overflow-{index}"), peer_session_id: live[peer].session_id.clone(), ..proposal.clone() };
        let pending = links::propose(&mut connection, &live[2], &proposal, &live, 700_000).unwrap().result.unwrap();
        let approval = link_approval(&connection, &pending, &format!("overflow-{index}"));
        assert_eq!(store::control(&mut connection, &approval, &live, 700_001).unwrap_err().code, ErrorCode::Limit);
    }
    assert_eq!(connection.query_row("SELECT count(*) FROM links WHERE state='active'", [], |row| row.get::<_, i64>(0)).unwrap(), 10);
}

#[test]
fn edited_link_proposals_survive_restart_and_need_the_new_exact_confirmation() {
    if isolated("edited_link_proposals_survive_restart_and_need_the_new_exact_confirmation") { return; }
    let (mut connection, _, live, proposal) = proposal_fixture();
    // The work may be owned by the peer, rather than the requester.
    let proposal = LinkProposal { peer_session_id: live[0].session_id.clone(), ..proposal };
    let pending = links::propose(&mut connection, &live[1], &proposal, &live, 1000).unwrap().result.unwrap();
    let old_approval = link_approval(&connection, &pending, "old-approval");
    let replacement = LinkProposal { purpose: "Revised coordination purpose".into(), ..proposal.clone() };
    let edit = host_action(json!({"action":"link_edit","request_id":"edit","proposal_id":pending["proposal_id"],"expected_revision":1,"replacement":replacement}));
    let revised = store::control(&mut connection, &edit, &live, 2000).unwrap().result.unwrap();
    assert_eq!(revised["revision"], 2);
    assert_eq!(revised["task"], json!(proposal.task));
    assert_eq!(revised["purpose"], "Revised coordination purpose");
    assert_eq!(revised["participants"], json!([live[1], live[0]]));
    assert_eq!(revised["permission"], "coordination_messages_only");
    drop(connection);
    let mut connection = store::open(&cockpit::paths::team_database()).unwrap();
    assert_eq!(store::control(&mut connection, &edit, &[], 3000).unwrap().result.unwrap(), revised);
    let replay = links::propose(&mut connection, &live[1], &proposal, &live, 3000).unwrap().result.unwrap();
    assert_eq!(replay, json!({"proposal_id":pending["proposal_id"],"revision":1,"status":"closed"}));
    assert!(store::control(&mut connection, &old_approval, &live, 3000).is_err());
    let approval = link_approval(&connection, &revised, "revised-approval");
    store::control(&mut connection, &approval, &live, 4000).unwrap();
    assert_eq!(connection.query_row("SELECT purpose FROM link_proposals WHERE id=?1", [pending["proposal_id"].as_str().unwrap()], |row| row.get::<_, String>(0)).unwrap(), "Coordinate shared work");
    assert_eq!(connection.query_row("SELECT parent_id FROM link_proposals WHERE id=?1", [revised["proposal_id"].as_str().unwrap()], |row| row.get::<_, String>(0)).unwrap(), pending["proposal_id"].as_str().unwrap());
}

#[test]
fn link_controls_fail_closed_on_bad_todo_state_and_lost_lifecycle_history() {
    if isolated("link_controls_fail_closed_on_bad_todo_state_and_lost_lifecycle_history") { return; }
    let (mut connection, mut todos, live, proposal) = proposal_fixture();
    let path = cockpit::paths::todo();
    let original = fs::read(&path).unwrap();
    let pending = links::propose(&mut connection, &live[0], &proposal, &live, 1000).unwrap().result.unwrap();
    let approval = link_approval(&connection, &pending, "approve");
    for document in ["{broken".into(), json!({"items":[]}).to_string(),
        json!({"items":[{"id":proposal.task.task_id,"project":"first","cwd":live[0].project.trim_start_matches("path:"),"text":"Joint work","status":"proposed"}]}).to_string(),
        json!({"items":[{"id":proposal.task.task_id,"project":"first","cwd":live[0].project.trim_start_matches("path:"),"text":"Joint work","status":"done"}]}).to_string()] {
        fs::write(&path, document).unwrap();
        assert!(store::control(&mut connection, &approval, &live, 2000).is_err());
        let fresh = LinkProposal { submission_key: "fresh".into(), ..proposal.clone() };
        assert!(links::propose(&mut connection, &live[0], &fresh, &live, 2000).is_err());
    }
    fs::write(&path, original).unwrap();
    assert!(todos.set_status(&proposal.task.task_id, "done", ""));
    assert!(todos.set_status(&proposal.task.task_id, "todo", ""));
    assert!(store::control(&mut connection, &approval, &live, 3000).is_err());
    let fresh = LinkProposal { submission_key: "fresh".into(), ..proposal.clone() };
    let pending = links::propose(&mut connection, &live[0], &fresh, &live, 3000).unwrap().result.unwrap();
    let approval = link_approval(&connection, &pending, "new-generation");
    store::control(&mut connection, &approval, &live, 4000).unwrap();
    assert_eq!(connection.query_row("SELECT work_generation FROM links", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    connection.execute_batch("PRAGMA foreign_keys=OFF; DROP TRIGGER retain_link_work; DELETE FROM link_work;").unwrap();
    let lost = LinkProposal { submission_key: "lost".into(), ..proposal };
    assert!(links::propose(&mut connection, &live[0], &lost, &live, 5000).is_err());
    assert_eq!(connection.query_row("SELECT count(*) FROM link_work", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

fn checkout(path: &Path) -> String {
    fs::create_dir_all(path.join(".git")).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn shared_work_requires_an_existing_approved_typed_todo() {
    if std::env::var_os("SESSION_LINKS_LOOKUP_CHILD").is_none() {
        let sandbox = Sandbox::new();
        let output = sandbox.command(std::env::current_exe().unwrap(), &[("SESSION_LINKS_LOOKUP_CHILD", "1")])
            .args(["--exact", "shared_work_requires_an_existing_approved_typed_todo", "--nocapture"]).output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    let home = cockpit::paths::home();
    let primary = checkout(&home.join("IdeaProjects/project"));
    let other = checkout(&home.join("other/project"));
    let clone = checkout(&cockpit::paths::clones_dir().join("project/feature"));
    let task = TaskKey { project: format!("path:{primary}"), task_id: "1234abcd".into() };
    let item = json!({"id": task.task_id, "project": "project", "cwd": primary, "text": "Joint work", "status": "doing"});
    let path = cockpit::paths::todo();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut cases = vec![("unknown", json!({"items": []})), ("duplicate", json!({"items": [item.clone(), item.clone()]})),
        ("wrong document", json!({"items": {}}))];
    for (name, field, value) in [
        ("proposed", "status", json!("proposed")), ("done", "status", json!("done")),
        ("invalid status", "status", json!("unknown")), ("missing status", "status", json!(null)),
        ("owner mismatch", "project", json!("elsewhere")), ("same basename", "cwd", json!(other)),
        ("missing checkout", "cwd", json!("/nonexistent/shared-work-checkout")), ("empty title", "text", json!("")),
    ] {
        let mut changed = item.clone();
        changed[field] = value;
        cases.push((name, json!({"items": [changed]})));
    }
    for (name, document) in cases {
        fs::write(&path, document.to_string()).unwrap();
        assert!(todo::read_shared_work(&task).is_err(), "accepted {name}");
    }
    fs::write(&path, "{broken").unwrap();
    assert!(todo::read_shared_work(&task).is_err());
    fs::remove_file(&path).unwrap();
    assert!(todo::read_shared_work(&task).is_err());
    fs::create_dir(&path).unwrap();
    assert!(todo::read_shared_work(&task).is_err());
    fs::remove_dir(&path).unwrap();
    for cwd in [primary.as_str(), clone.as_str()] {
        let mut valid = item.clone();
        valid["cwd"] = json!(cwd);
        fs::write(&path, json!({"items": [valid]}).to_string()).unwrap();
        let work = todo::read_shared_work(&task).unwrap();
        assert_eq!(work.task, task);
        assert_eq!(work.status, "doing");
        assert_eq!(work.title, "Joint work");
    }
    let issue = TaskKey { task_id: "owner/repo#12".into(), ..task.clone() };
    assert!(todo::read_shared_work(&issue).is_err());
    let basename = TaskKey { project: "project".into(), ..task };
    assert!(todo::read_shared_work(&basename).is_err());
    assert_eq!(teams::canonical_project(&clone).unwrap(), format!("path:{primary}"));
    assert_ne!(teams::canonical_project(&other).unwrap(), format!("path:{primary}"));
}

fn version_two() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA user_version=2;
        CREATE TABLE teams (task_key TEXT PRIMARY KEY, project TEXT NOT NULL, task_id TEXT NOT NULL, revision INTEGER NOT NULL);
        CREATE TABLE proposals (id TEXT PRIMARY KEY, sender TEXT NOT NULL, submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL,
            task_key TEXT NOT NULL, project TEXT NOT NULL, task_id TEXT NOT NULL, mapping TEXT NOT NULL, mapping_hash TEXT NOT NULL,
            reason TEXT NOT NULL, revision INTEGER NOT NULL, status TEXT NOT NULL, created_at INTEGER NOT NULL,
            parent_id TEXT, created_by TEXT NOT NULL DEFAULT 'agent', recruits TEXT NOT NULL DEFAULT '[]', spawn_agent TEXT NOT NULL DEFAULT '', UNIQUE(sender,submission_key));
        CREATE TABLE members (task_key TEXT NOT NULL, session_id TEXT NOT NULL UNIQUE, role TEXT NOT NULL, agent TEXT NOT NULL,
            runtime TEXT NOT NULL, epoch TEXT NOT NULL, last_idle_turn TEXT, PRIMARY KEY(task_key,role));
        CREATE TABLE messages (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, task_key TEXT NOT NULL,
            sender TEXT NOT NULL, recipient TEXT NOT NULL, sender_epoch TEXT NOT NULL, recipient_epoch TEXT NOT NULL,
            submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL, envelope TEXT NOT NULL, state TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL, created_at INTEGER NOT NULL,
            last_error TEXT NOT NULL DEFAULT '', idle_turn TEXT, recipient_runtime TEXT NOT NULL, UNIQUE(sender,submission_key));
        CREATE TABLE attempts (id TEXT PRIMARY KEY, message_id TEXT NOT NULL REFERENCES messages(id), number INTEGER NOT NULL,
            state TEXT NOT NULL, native_id TEXT, finished_at INTEGER, started_at INTEGER NOT NULL DEFAULT 0, UNIQUE(message_id,number));
        INSERT INTO teams VALUES ('task','path:/project','1234abcd',7);
        INSERT INTO proposals (id,sender,submission_key,fingerprint,task_key,project,task_id,mapping,mapping_hash,reason,revision,status,created_at)
            VALUES ('proposal','sender','request','fingerprint','task','path:/project','1234abcd','[]','hash','work',7,'approved',10);
        INSERT INTO members VALUES ('task','sender','developer','claude','runtime','epoch',NULL);
        INSERT INTO messages (id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,attempts,next_at,created_at,recipient_runtime)
            VALUES ('message','task','sender','recipient','epoch','peer-epoch','submission','fingerprint','{}','received',1,20,10,'peer-runtime');
        INSERT INTO messages (id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime)
            VALUES ('pending','task','sender','recipient','epoch','peer-epoch','queued-submission','pending-fingerprint','{\"body\":\"queued\"}','queued',30,25,'peer-runtime');
        INSERT INTO attempts VALUES ('attempt','message',1,'received','native-receipt',20,15);").unwrap();
    connection
}

fn team_state(connection: &Connection) -> Vec<Vec<String>> {
    ["teams", "proposals", "members", "attempts", "messages"].into_iter().map(|table| {
        let columns = if table == "messages" {
            "seq,id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,attempts,next_at,created_at,last_error,idle_turn,recipient_runtime"
        } else { "*" };
        let mut statement = connection.prepare(&format!("SELECT {columns} FROM {table}")).unwrap();
        let columns = statement.column_count();
        statement.query_map([], |row| Ok((0..columns).map(|column| format!("{:?}", row.get_ref(column).unwrap())).collect::<Vec<_>>().join("|")))
            .unwrap().collect::<Result<Vec<_>, _>>().unwrap()
    }).collect()
}

#[test]
fn link_schema_upgrade_preserves_team_state_and_receipts() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("protected/teams.sqlite");
    fs::create_dir(path.parent().unwrap()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    let fixture = version_two();
    fixture.execute("VACUUM INTO ?1", [path.to_str().unwrap()]).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let team_before = team_state(&fixture);
    let connection = store::open(&path).unwrap();
    let team_after = team_state(&connection);
    assert_eq!(team_before, team_after);
    drop(connection);
    let connection = store::open(&path).unwrap();
    assert_eq!(team_before, team_state(&connection));
    let receipt: (String, String, Option<String>) = connection.query_row(
        "SELECT state,scope_kind,link_id FROM messages WHERE id='message'", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    assert_eq!(receipt, ("received".into(), "team".into(), None));
    assert!(connection.execute("INSERT INTO messages (id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime)
        SELECT 'new-id',task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime FROM messages WHERE id='message'", []).is_err());
}

#[test]
fn failed_or_future_link_migrations_do_not_partially_upgrade() {
    let mut connection = version_two();
    connection.execute_batch("CREATE TABLE links (wrong TEXT);").unwrap();
    assert!(store::migrate(&mut connection).is_err());
    assert_eq!(connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
    assert_eq!(connection.query_row("SELECT count(*) FROM pragma_table_info('messages') WHERE name='scope_kind'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    let connection = version_two();
    connection.execute_batch("PRAGMA user_version=999;").unwrap();
    assert!(links::migrate(&connection).is_err());
    assert_eq!(connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 999);
}

fn approved_link_fixture(connection: &Connection) {
    connection.execute_batch("INSERT INTO link_work VALUES ('shared','path:/project','abcdef12',4,'open',10);
        INSERT INTO link_proposals (id,sender,submission_key,fingerprint,task_key,purpose,revision,mapping_hash,work_generation,status,created_at)
            VALUES ('p1','a','request-1','fingerprint','shared','Joint work',1,'hash-1',4,'approved',10);
        INSERT INTO link_participants VALUES ('p1',0,'a','claude','path:/project','runtime-a');
        INSERT INTO link_participants VALUES ('p1',1,'b','opencode','path:/other','runtime-b');
        INSERT INTO links VALUES ('link-1','p1',1,'hash-1','shared',4,'a','b','epoch-1','active',NULL,20,NULL);").unwrap();
}

#[test]
fn link_schema_binds_exact_immutable_approval_and_one_active_unordered_pair() {
    let mut connection = version_two();
    store::migrate(&mut connection).unwrap();
    approved_link_fixture(&connection);
    for statement in [
        "UPDATE link_proposals SET mapping_hash='changed' WHERE id='p1'",
        "UPDATE link_proposals SET revision=2 WHERE id='p1'",
        "UPDATE link_participants SET runtime='replacement' WHERE proposal_id='p1'",
        "DELETE FROM link_participants WHERE proposal_id='p1'",
        "UPDATE links SET revision=2 WHERE id='link-1'",
        "DELETE FROM link_work WHERE task_key='shared'",
        "UPDATE link_work SET generation=3 WHERE task_key='shared'",
    ] {
        assert!(connection.execute(statement, []).is_err(), "accepted {statement}");
    }
    connection.execute_batch("INSERT INTO link_proposals (id,sender,submission_key,fingerprint,task_key,purpose,revision,mapping_hash,work_generation,status,created_at)
        VALUES ('p2','b','request-2','fingerprint','shared','Joint work',2,'hash-2',4,'approved',30);
        INSERT INTO link_participants VALUES ('p2',0,'b','opencode','path:/other','runtime-b');
        INSERT INTO link_participants VALUES ('p2',1,'a','claude','path:/project','runtime-a');").unwrap();
    for (low, high, hash) in [("b", "a", "hash-2"), ("a", "b", "hash-1"), ("a", "c", "hash-2"), ("a", "b", "hash-2")] {
        assert!(connection.execute("INSERT INTO links VALUES ('link-2','p2',2,?1,'shared',4,?2,?3,'epoch-2','active',NULL,30,NULL)",
            (hash, low, high)).is_err());
    }
    connection.execute("UPDATE links SET state='closed',closure_reason='done',closed_at=40 WHERE id='link-1'", []).unwrap();
    connection.execute("INSERT INTO links VALUES ('link-2','p2',2,'hash-2','shared',4,'a','b','epoch-2','active',NULL,50,NULL)", []).unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM teams", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
}

#[test]
fn store_open_rolls_back_legacy_and_link_changes_on_migration_failure() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("protected/teams.sqlite");
    fs::create_dir(path.parent().unwrap()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    let fixture = version_two();
    fixture.execute_batch("CREATE TABLE links (wrong TEXT); PRAGMA user_version=1; ALTER TABLE proposals DROP COLUMN spawn_agent;").unwrap();
    fixture.execute("VACUUM INTO ?1", [path.to_str().unwrap()]).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(store::open(&path).is_err());
    let connection = Connection::open(&path).unwrap();
    assert_eq!(connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(connection.query_row("SELECT count(*) FROM pragma_table_info('proposals') WHERE name='spawn_agent'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(connection.query_row("SELECT count(*) FROM sqlite_master WHERE name='link_work'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

fn isolated(name: &str) -> bool {
    if std::env::var_os("SESSION_LINKS_LIFECYCLE_CHILD").is_some() { return false; }
    let sandbox = Sandbox::new();
    let output = sandbox.command(std::env::current_exe().unwrap(), &[("SESSION_LINKS_LIFECYCLE_CHILD", "1")])
        .args(["--exact", name, "--nocapture"]).output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    true
}

fn lifecycle_fixture() -> (Connection, todo::TodoList, TaskKey) {
    let project = checkout(&cockpit::paths::home().join("IdeaProjects/project"));
    let mut todos = todo::TodoList::load();
    let id = todos.add("project", &project, "Joint work");
    let task = TaskKey { project: format!("path:{project}"), task_id: id };
    let connection = store::open(&cockpit::paths::team_database()).unwrap();
    link_fixture::approve(&connection, &task.project, &task.task_id);
    (connection, todos, task)
}

#[test]
fn lifecycle_checks_reject_old_approvals_and_missing_history() {
    if isolated("lifecycle_checks_reject_old_approvals_and_missing_history") { return; }
    let (mut connection, mut todos, task) = lifecycle_fixture();
    assert!(todos.set_status(&task.task_id, "done", ""));
    assert!(todos.set_status(&task.task_id, "todo", ""));
    let approve_old_proposal = |connection: &mut Connection| {
        let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        links::require_open_work(&transaction, &task, 4)?;
        transaction.execute("INSERT INTO links SELECT 'old-approval',id,revision,mapping_hash,task_key,work_generation,'a','b','old-epoch','active',NULL,30,NULL
            FROM link_proposals WHERE id=?1", [format!("{}-pending", task.task_id)])?;
        transaction.commit().map_err(teams::TeamError::from)
    };
    assert!(approve_old_proposal(&mut connection).is_err());
    assert_eq!(link_fixture::state(&connection, &task.task_id), "closed");
    connection.execute_batch("PRAGMA foreign_keys=OFF; DROP TRIGGER retain_link_work; DELETE FROM link_work;").unwrap();
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    assert!(links::enroll_work(&transaction, &task).is_err());
    assert!(links::require_open_work(&transaction, &task, 4).is_err());
    drop(transaction);
    assert!(links::reopen_work(&mut connection, &task).is_err());
}

#[test]
fn lifecycle_reads_authoritative_todo_after_acquiring_write_boundary() {
    if isolated("lifecycle_reads_authoritative_todo_after_acquiring_write_boundary") { return; }
    let (mut connection, _, task) = lifecycle_fixture();
    let snapshot = todo::read_shared_work(&task).unwrap();
    let path = cockpit::paths::todo();
    for document in ["{broken".to_string(), "{}".to_string(), json!({"items": []}).to_string(),
        json!({"items": [{"id": task.task_id, "project": "project", "cwd": snapshot.task.project.trim_start_matches("path:"), "text": "Joint work", "status": "done"}]}).to_string()] {
        fs::write(&path, document).unwrap();
        let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
        assert!(links::require_open_work(&transaction, &task, 4).is_err());
        assert!(links::enroll_work(&transaction, &task).is_err());
        drop(transaction);
        assert!(links::reopen_work(&mut connection, &task).is_err());
    }
}

#[test]
fn lifecycle_closure_preserves_uncertain_deliveries_and_other_work() {
    if isolated("lifecycle_closure_preserves_uncertain_deliveries_and_other_work") { return; }
    let (mut connection, _, task) = lifecycle_fixture();
    connection.execute("UPDATE messages SET recipient_runtime='other-runtime'", []).unwrap();
    let other = TaskKey { task_id: "abcdef12".into(), ..task.clone() };
    link_fixture::approve(&connection, &other.project, &other.task_id);
    connection.execute("INSERT INTO teams VALUES ('team','path:/unrelated','1234abcd',7)", []).unwrap();
    links::invalidate_work(&mut connection, &task, "done").unwrap();
    assert_eq!(link_fixture::state(&connection, &other.task_id), "active");
    for (initial, expected) in [("queued", "cancelled"), ("retry", "cancelled"), ("ready", "cancelled"), ("submitting", "submitting"), ("uncertain", "uncertain"), ("received", "received")] {
        let state: String = connection.query_row("SELECT state FROM messages WHERE id=?1", [format!("{}-{initial}", task.task_id)], |row| row.get(0)).unwrap();
        assert_eq!(state, expected);
    }
    assert_eq!(connection.query_row("SELECT state FROM attempts WHERE id=?1", [format!("{}-attempt", task.task_id)], |row| row.get::<_, String>(0)).unwrap(), "not_submitted");
    assert_eq!(connection.query_row("SELECT revision FROM teams WHERE task_key='team'", [], |row| row.get::<_, i64>(0)).unwrap(), 7);
}

#[test]
fn lifecycle_enrollment_cannot_reset_a_committed_completion() {
    if isolated("lifecycle_enrollment_cannot_reset_a_committed_completion") { return; }
    let project = checkout(&cockpit::paths::home().join("IdeaProjects/project"));
    let mut todos = todo::TodoList::load();
    let id = todos.add("project", &project, "New shared work");
    let task = TaskKey { project: format!("path:{project}"), task_id: id };
    let mut connection = store::open(&cockpit::paths::team_database()).unwrap();
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    let (work, generation) = links::enroll_work(&transaction, &task).unwrap();
    assert_eq!(work.title, "New shared work");
    assert_eq!(generation, 0);
    transaction.commit().unwrap();
    links::invalidate_work(&mut connection, &task, "done").unwrap();
    // The JSON still says todo, as it would after a crash between invalidation and its save.
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    assert!(links::enroll_work(&transaction, &task).is_err());
    assert!(links::require_open_work(&transaction, &task, 0).is_err());
    transaction.commit().unwrap();
    assert!(todos.set_status(&task.task_id, "todo", ""));
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    assert_eq!(links::enroll_work(&transaction, &task).unwrap().1, 1);
    assert!(links::require_open_work(&transaction, &task, 0).is_err());
    assert!(links::require_open_work(&transaction, &task, 1).is_ok());
}

#[test]
fn lifecycle_authorization_waits_for_writer_before_reading_todo() {
    if isolated("lifecycle_authorization_waits_for_writer_before_reading_todo") { return; }
    use std::sync::atomic::{AtomicBool, Ordering};
    static WAITING: AtomicBool = AtomicBool::new(false);
    let (mut connection, _, task) = lifecycle_fixture();
    for enrollment in [false, true] {
        WAITING.store(false, Ordering::SeqCst);
        let original = fs::read(cockpit::paths::todo()).unwrap();
        let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
        let task = task.clone();
        let path = cockpit::paths::team_database();
        let reader = std::thread::spawn(move || {
            let mut connection = Connection::open(path).unwrap();
            connection.busy_handler(Some(|_| {
                WAITING.store(true, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(1));
                true
            })).unwrap();
            // Even a deferred caller must acquire the writer boundary before reading JSON.
            let transaction = connection.transaction().unwrap();
            if enrollment { links::enroll_work(&transaction, &task).map(|_| ()) }
            else { links::require_open_work(&transaction, &task, 4).map(|_| ()) }
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !WAITING.load(Ordering::SeqCst) && !reader.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let waited = WAITING.load(Ordering::SeqCst);
        fs::write(cockpit::paths::todo(), "{broken").unwrap();
        transaction.commit().unwrap();
        assert!(reader.join().unwrap().is_err(), "authorized a pre-transaction snapshot");
        assert!(waited, "authorization bypassed the lifecycle writer");
        fs::write(cockpit::paths::todo(), original).unwrap();
    }
}

#[test]
fn lifecycle_invalidation_and_final_authorization_share_the_writer_boundary() {
    if isolated("lifecycle_invalidation_and_final_authorization_share_the_writer_boundary") { return; }
    let (mut connection, _, task) = lifecycle_fixture();
    let mut closer = store::open(&cockpit::paths::team_database()).unwrap();
    closer.busy_timeout(std::time::Duration::ZERO).unwrap();
    let claim = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    links::require_open_work(&claim, &task, 4).unwrap();
    assert!(links::invalidate_work(&mut closer, &task, "done").is_err());
    claim.commit().unwrap();
    links::invalidate_work(&mut closer, &task, "done").unwrap();
    let claim = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    assert!(links::require_open_work(&claim, &task, 4).is_err());
}

#[test]
fn lifecycle_host_mutation_guard_is_shared_across_processes() {
    if let Ok(operation) = std::env::var("LIFECYCLE_GUARD_OPERATION") {
        let task: TaskKey = serde_json::from_str(&std::env::var("LIFECYCLE_GUARD_TASK").unwrap()).unwrap();
        let mut connection = store::open(&cockpit::paths::team_database()).unwrap();
        match operation.as_str() {
            "reopen" => assert!(links::reopen_work(&mut connection, &task).is_err()),
            "invalidate" => assert!(links::invalidate_work(&mut connection, &task, "done").is_err()),
            "save" => assert!(!todo::TodoList::load().set_status(&task.task_id, "done", "")),
            _ => unreachable!(),
        }
        return;
    }
    if isolated("lifecycle_host_mutation_guard_is_shared_across_processes") { return; }
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    let (mut connection, _, task) = lifecycle_fixture();
    links::invalidate_work(&mut connection, &task, "done").unwrap();
    let before = fs::read(cockpit::paths::todo()).unwrap();
    let guard = fs::OpenOptions::new().create(true).write(true).truncate(false).mode(0o600)
        .open(cockpit::paths::todo().with_extension("mutation.lock")).unwrap();
    assert_eq!(unsafe { libc::flock(guard.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
    let mut failures = Vec::new();
    for operation in ["reopen", "invalidate", "save"] {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .env("LIFECYCLE_GUARD_OPERATION", operation).env("LIFECYCLE_GUARD_TASK", serde_json::to_string(&task).unwrap())
            .args(["--exact", "lifecycle_host_mutation_guard_is_shared_across_processes", "--nocapture"])
            .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
        let output = common::sandbox::finish(child, std::time::Duration::from_secs(5));
        if !output.status.success() {
            failures.push(format!("{operation}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)));
        }
    }
    drop(guard);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(fs::read(cockpit::paths::todo()).unwrap(), before);
    assert_eq!(connection.query_row("SELECT generation FROM link_work", [], |row| row.get::<_, i64>(0)).unwrap(), 5);
    links::reopen_work(&mut connection, &task).unwrap();
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    assert!(links::require_open_work(&transaction, &task, 5).is_ok());
}
