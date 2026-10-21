//! The Team view's content: what Theo reads before approving, built from the daemon's snapshot.

use cockpit::teams::view::{Row, TeamView};
use serde_json::{Value, json};

const DESIGNER: &str = "aaaaaaaa-0000-4000-8000-000000000000";
const DEVELOPER: &str = "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L";
const QA: &str = "bbbbbbbb-0000-4000-8000-000000000000";
const PROJECT: &str = "remote:git@github.com:example/project-a.git";

fn snapshot() -> Value {
    let mapping = json!([{"session_id": DESIGNER, "role": "designer", "agent": "claude", "runtime": "r1"},
        {"session_id": DEVELOPER, "role": "developer", "agent": "opencode", "runtime": "r2"},
        {"session_id": QA, "role": "qa", "agent": "claude", "runtime": "r3"}]).to_string();
    let envelope = json!({"message_id": "m1", "kind": "notice", "body": "Frame ready\u{1b}[2J",
        "task": {"project": PROJECT, "task_id": "1d3fd318"}, "sender": {"session_id": DEVELOPER, "role": "developer", "agent": "opencode"},
        "recipient": {"session_id": DESIGNER, "role": "designer", "agent": "claude"}}).to_string();
    json!({
        "proposals": [{"proposal_id": "p1", "project": PROJECT, "task_id": "1d3fd318", "mapping": mapping, "mapping_hash": "h1",
            "revision": 2, "status": "pending", "reason": "Review this task", "sender": DEVELOPER, "created_by": "agent", "parent_id": null},
            {"proposal_id": "p0", "project": PROJECT, "task_id": "1d3fd318", "mapping": "[]", "mapping_hash": "h0", "revision": 1,
             "status": "superseded", "reason": "old", "sender": DEVELOPER, "created_by": "agent", "parent_id": null}],
        "members": [{"session_id": QA, "role": "qa", "agent": "claude", "project": PROJECT, "task_id": "0123abcd", "runtime": "r", "epoch": "e"}],
        "messages": [{"seq": 1, "message_id": "m1", "envelope": envelope, "state": "uncertain", "attempts": 1, "next_at": 0, "error": "no receipt"}],
        "escalations": [{"thread_id": "t1", "reason": "three clarification rounds ended unresolved", "status": "open", "note": "", "created_at": 0}],
        "sessions": [{"session_id": DESIGNER, "agent": "claude", "project": PROJECT, "project_name": "project-a", "label": "Design review",
            "state": "waiting"}],
    })
}

#[test]
fn pending_requests_show_the_full_exact_mapping_before_approval() {
    let view = TeamView::new(snapshot());
    assert_eq!(view.selected_row(), Some(Row::Proposal(0)));
    let detail = view.detail().join("\n");
    for expected in [PROJECT, "1d3fd318", &format!("designer: {DESIGNER}"), &format!("developer: {DEVELOPER}"), &format!("qa: {QA}"),
                     "Revision 2", "Review this task", "grants no tool, scope, install or Git permission"] {
        assert!(detail.contains(expected), "missing {expected:?} in\n{detail}");
    }
}

#[test]
fn superseded_requests_are_not_offered_for_approval() {
    let view = TeamView::new(snapshot());
    let proposals = view.rows().into_iter().filter(|row| matches!(row, Row::Proposal(_))).count();
    assert_eq!(proposals, 1);
}

#[test]
fn the_overview_lists_members_escalations_and_uncertain_deliveries() {
    let view = TeamView::new(snapshot());
    assert_eq!(view.rows(), [Row::Proposal(0), Row::Member(0), Row::Escalation(0), Row::Uncertain(0)]);
    assert_eq!(TeamView::attention(&snapshot()), 3);
}

#[test]
fn uncertain_deliveries_offer_reconciliation_never_a_resend() {
    let mut view = TeamView::new(snapshot());
    view.selected = 3;
    let detail = view.detail().join("\n").to_lowercase();
    assert!(detail.contains("h it arrived") && detail.contains("c cancel"), "{detail}");
    assert!(!detail.contains("resend ") && !detail.contains("retry"), "{detail}");
}

#[test]
fn message_text_cannot_rewrite_the_terminal() {
    let mut view = TeamView::new(snapshot());
    view.is_history = true;
    view.selected = 0;
    let shown = view.rows().iter().map(|row| view.row_text(*row)).chain(view.detail()).collect::<Vec<_>>().join("\n");
    assert!(!shown.contains('\u{1b}'), "{shown:?}");
    assert!(shown.contains("\\u001b"), "{shown}");
}

#[test]
fn moving_the_selection_stays_inside_the_rows() {
    let mut view = TeamView::new(snapshot());
    view.move_by(-5);
    assert_eq!(view.selected, 0);
    view.move_by(50);
    assert_eq!(view.selected, 3);
}

#[test]
fn failed_spawns_count_as_needing_theo() {
    let mut state = snapshot();
    state["spawns"] = json!([{"token": "t1", "role": "designer", "agent": "claude", "status": "failed", "directory": "/p", "created_at": 1}]);
    assert_eq!(TeamView::attention(&state), TeamView::attention(&snapshot()) + 1);
}

fn with_recruit_request() -> Value {
    let mut state = snapshot();
    let mapping = json!([{"session_id": DEVELOPER, "role": "developer", "agent": "opencode", "runtime": "r2"}]).to_string();
    let recruits = json!([{"role": "designer", "brief": "Design the jobs list\u{1b}[2J"}, {"role": "qa", "brief": "Check contrast"}]).to_string();
    state["proposals"] = json!([{"proposal_id": "p2", "project": PROJECT, "task_id": "1d3fd318", "mapping": mapping, "mapping_hash": "h2",
        "revision": 1, "status": "pending", "reason": "The UI needs a design", "sender": DEVELOPER, "created_by": "agent", "parent_id": null,
        "recruits": recruits, "spawn_agent": "claude"}]);
    state
}

#[test]
fn a_recruit_request_shows_the_asker_each_new_teammate_its_agent_and_brief() {
    let view = TeamView::new(with_recruit_request());
    let detail = view.detail().join("\n");
    for expected in [&format!("asker (developer): {DEVELOPER}"), "new designer (claude): Design the jobs list",
                     "QA: a fresh reviewer for each handoff (claude): Check contrast"] {
        assert!(detail.contains(expected), "missing {expected:?} in\n{detail}");
    }
    assert!(!detail.contains('\u{1b}') && detail.contains("\\u001b"), "{detail}");
}

#[test]
fn a_failed_spawn_is_listed_and_counted() {
    let mut state = snapshot();
    state["spawns"] = json!([{"token": "t1", "task_key": "k", "role": "designer", "agent": "claude", "status": "failed", "directory": "/p", "created_at": 1}]);
    let view = TeamView::new(state.clone());
    let texts: Vec<String> = view.rows().iter().map(|row| view.row_text(*row)).collect();
    assert!(texts.iter().any(|text| text.contains("did not start") && text.contains("designer")), "{texts:?}");
    assert_eq!(TeamView::attention(&state), TeamView::attention(&snapshot()) + 1);
}
