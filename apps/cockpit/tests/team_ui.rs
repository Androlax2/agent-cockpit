//! The Team view's content: what Theo reads before approving, built from the daemon's snapshot.

use cockpit::teams::view::{Row, TeamView};
use cockpit::{
    sessions::Session,
    teams::{
        TaskKey,
        sidebar::{self, SidebarRow},
    },
};
use serde_json::{Value, json};
use std::collections::HashSet;

const DESIGNER: &str = "aaaaaaaa-0000-4000-8000-000000000000";
const DEVELOPER: &str = "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L";
const QA: &str = "bbbbbbbb-0000-4000-8000-000000000000";
const PROJECT: &str = "remote:git@github.com:example/project-a.git";

fn snapshot() -> Value {
    let mapping =
        json!([{"session_id": DESIGNER, "role": "designer", "agent": "claude", "runtime": "r1"},
        {"session_id": DEVELOPER, "role": "developer", "agent": "opencode", "runtime": "r2"},
        {"session_id": QA, "role": "qa", "agent": "claude", "runtime": "r3"}])
        .to_string();
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
    for expected in [
        PROJECT,
        "1d3fd318",
        &format!("designer: {DESIGNER}"),
        &format!("developer: {DEVELOPER}"),
        &format!("qa: {QA}"),
        "Revision 2",
        "Review this task",
        "grants no tool, scope, install or Git permission",
    ] {
        assert!(
            detail.contains(expected),
            "missing {expected:?} in\n{detail}"
        );
    }
}

#[test]
fn superseded_requests_are_not_offered_for_approval() {
    let view = TeamView::new(snapshot());
    let proposals = view
        .rows()
        .into_iter()
        .filter(|row| matches!(row, Row::Proposal(_)))
        .count();
    assert_eq!(proposals, 1);
}

#[test]
fn the_overview_lists_members_escalations_and_uncertain_deliveries() {
    let view = TeamView::new(snapshot());
    assert_eq!(
        view.rows(),
        [
            Row::Proposal(0),
            Row::Member(0),
            Row::Escalation(0),
            Row::Uncertain(0)
        ]
    );
    assert_eq!(TeamView::attention(&snapshot()), 3);
}

#[test]
fn uncertain_deliveries_offer_reconciliation_never_a_resend() {
    let mut view = TeamView::new(snapshot());
    view.selected = 3;
    let detail = view.detail().join("\n").to_lowercase();
    assert!(
        detail.contains("h it arrived") && detail.contains("c cancel"),
        "{detail}"
    );
    assert!(
        !detail.contains("resend ") && !detail.contains("retry"),
        "{detail}"
    );
}

#[test]
fn message_text_cannot_rewrite_the_terminal() {
    let mut view = TeamView::new(snapshot());
    view.is_history = true;
    view.selected = 0;
    let shown = view
        .rows()
        .iter()
        .map(|row| view.row_text(*row))
        .chain(view.detail())
        .collect::<Vec<_>>()
        .join("\n");
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
fn chief_review_records_do_not_change_team_navigation_or_attention() {
    let mut state = snapshot();
    state["records"] = json!([{"kind":"decision","state":"open","id":"chief-decision"},
        {"kind":"proposal","freshness":"pending","id":"chief-proposal"}]);
    let view = TeamView::new(state.clone());
    assert_eq!(TeamView::attention(&state), 3);
    assert_eq!(
        view.rows(),
        [
            Row::Proposal(0),
            Row::Member(0),
            Row::Escalation(0),
            Row::Uncertain(0)
        ]
    );
}

#[test]
fn failed_spawns_count_as_needing_theo() {
    let mut state = snapshot();
    state["spawns"] = json!([{"token": "t1", "role": "designer", "agent": "claude", "status": "failed", "directory": "/p", "created_at": 1}]);
    assert_eq!(
        TeamView::attention(&state),
        TeamView::attention(&snapshot()) + 1
    );
}

#[test]
fn failed_sleep_wakes_use_team_error_rows_and_attention() {
    for reason in [
        "could not start cockpit wake",
        "wake acknowledgement timed out\u{1b}[2J",
    ] {
        let mut state = snapshot();
        state["sleeps"] = json!([
            {"session_id": DESIGNER, "state": "failed", "error": reason},
            {"session_id": DEVELOPER, "state": "prepared", "error": ""},
            {"session_id": QA, "state": "bound", "error": ""}
        ]);
        assert_eq!(TeamView::attention(&state), 4);
        let mut view = TeamView::new(state);
        assert_eq!(view.rows().len(), 5);
        view.selected = 4;
        let text = view.row_text(view.selected_row().unwrap());
        assert!(text.starts_with("! ") && text.contains(DESIGNER), "{text}");
        let detail = view.detail().join("\n");
        assert!(
            detail.contains(reason.split('\u{1b}').next().unwrap()),
            "{detail}"
        );
        assert!(!detail.contains('\u{1b}'));
    }
}

#[test]
fn healthy_sleep_continuations_do_not_demand_attention() {
    for status in [
        "prepared",
        "launch_requested",
        "waking",
        "bound",
        "cancelled",
    ] {
        let mut state = snapshot();
        state["sleeps"] = json!([{"session_id":DESIGNER,"state":status,"error":""}]);
        assert_eq!(TeamView::attention(&state), 3, "{status}");
        assert_eq!(TeamView::new(state).rows().len(), 4, "{status}");
    }
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
    for expected in [
        &format!("asker (developer): {DEVELOPER}"),
        "new designer (claude): Design the jobs list",
        "QA: a fresh reviewer for each handoff (claude): Check contrast",
    ] {
        assert!(
            detail.contains(expected),
            "missing {expected:?} in\n{detail}"
        );
    }
    assert!(
        !detail.contains('\u{1b}') && detail.contains("\\u001b"),
        "{detail}"
    );
}

#[test]
fn a_failed_spawn_is_listed_and_counted() {
    let mut state = snapshot();
    state["spawns"] = json!([{"token": "t1", "task_key": "k", "role": "designer", "agent": "claude", "status": "failed", "directory": "/p", "created_at": 1}]);
    let view = TeamView::new(state.clone());
    let texts: Vec<String> = view.rows().iter().map(|row| view.row_text(*row)).collect();
    assert!(
        texts
            .iter()
            .any(|text| text.contains("did not start") && text.contains("designer")),
        "{texts:?}"
    );
    assert_eq!(
        TeamView::attention(&state),
        TeamView::attention(&snapshot()) + 1
    );
}

#[test]
fn a_recruit_request_is_summed_up_first_and_a_long_brief_cannot_push_it_away() {
    // Final review I6: a brief with many lines hid the rest of the request.
    let mut state = with_recruit_request();
    let recruits = json!([{"role": "designer", "brief": "line\n".repeat(40)}, {"role": "qa", "brief": "Check contrast"}]).to_string();
    state["proposals"][0]["recruits"] = json!(recruits);
    state["sessions"] = json!([{"session_id": DEVELOPER, "agent": "opencode", "project": PROJECT, "project_name": "project-a",
        "label": "", "state": "waiting", "cwd": "/home/theo/IdeaProjects/project-a"}]);
    let view = TeamView::new(state);
    let detail = view.detail();
    assert_eq!(
        detail[0],
        "Starts: new designer in /home/theo/IdeaProjects/project-a · a fresh QA at each handoff · agent claude"
    );
    let designer = detail
        .iter()
        .find(|line| line.starts_with("new designer"))
        .unwrap();
    assert!(
        !designer.contains('\n') && designer.contains("line⏎line"),
        "{designer}"
    );
}

#[test]
fn sidebar_keeps_same_task_ids_in_different_projects_and_resolves_launch_aliases() {
    let state = json!({"members":[
        {"session_id":DESIGNER,"project":"path:/a/shop","task_id":"1d3fd318","role":"designer"},
        {"session_id":DEVELOPER,"project":"path:/b/shop","task_id":"1d3fd318","role":"developer"}],
        "sessions":[{"session_id":DESIGNER,"project":"path:/a/shop","project_name":"shop"},
            {"session_id":DEVELOPER,"project":"path:/b/shop","project_name":"shop"}],"qa_plans":[]});
    let sessions = vec![
        Session {
            session: "launch-designer".into(),
            conversation: Some(DESIGNER.into()),
            project: "shop".into(),
            ..Session::default()
        },
        Session {
            session: DEVELOPER.into(),
            project: "shop".into(),
            ..Session::default()
        },
    ];
    let rows = sidebar::grouped_rows(&state, &sessions, &HashSet::new()).unwrap();
    assert_eq!(
        rows,
        [
            SidebarRow::Team {
                task: TaskKey {
                    project: "path:/a/shop".into(),
                    task_id: "1d3fd318".into()
                }
            },
            SidebarRow::Session {
                session_id: "launch-designer".into()
            },
            SidebarRow::Team {
                task: TaskKey {
                    project: "path:/b/shop".into(),
                    task_id: "1d3fd318".into()
                }
            },
            SidebarRow::Session {
                session_id: DEVELOPER.into()
            }
        ]
    );
}

#[test]
fn sidebar_findings_are_attached_to_the_current_handoff_not_interpreted_as_approval() {
    let task = TaskKey {
        project: PROJECT.into(),
        task_id: "1d3fd318".into(),
    };
    let mut state = json!({"handoffs":[{"project":PROJECT,"task_id":"1d3fd318","handoff_id":"h2","status":"current"}],
        "messages":[{"state":"delivered","envelope":json!({"kind":"findings","task":task,"handoff":{"id":"h1"},"body":"QA approved"}).to_string()}]});
    assert_eq!(sidebar::handoff_summary(&state, &task), "Handoff submitted");
    state["messages"][0]["envelope"] = json!(
        json!({"kind":"findings","task":task,"handoff":{"id":"h2"},"body":"QA approved"})
            .to_string()
    );
    assert_eq!(
        sidebar::handoff_summary(&state, &task),
        "QA findings received"
    );
    state.as_object_mut().unwrap().remove("handoffs");
    assert_eq!(
        sidebar::handoff_summary(&state, &task),
        "Handoff evidence unavailable"
    );
}

#[test]
fn sidebar_pending_qa_is_starting_not_a_second_planned_review() {
    let task = TaskKey {
        project: PROJECT.into(),
        task_id: "1d3fd318".into(),
    };
    let state = json!({"handoffs":[{"project":PROJECT,"task_id":"1d3fd318","handoff_id":"h2","status":"current","payload":{"commit":"abc"}}],
        "spawns":[{"task_key":serde_json::to_string(&task).unwrap(),"role":"qa","status":"pending","commit_id":"abc","created_at":1}]});
    assert_eq!(
        sidebar::qa_status(&state, &task),
        "QA starting · not yet linked"
    );
}

#[test]
fn sidebar_invalid_membership_is_unavailable_instead_of_dropping_the_session() {
    for member in [
        json!({"session_id":DESIGNER,"project":PROJECT,"task_id":"1d3fd318","role":"invented-role"}),
        json!({"session_id":DESIGNER,"project":PROJECT,"role":"designer"}),
    ] {
        let state = json!({"members":[member],"sessions":[]});
        assert!(
            sidebar::grouped_rows(
                &state,
                &[Session {
                    session: DESIGNER.into(),
                    project: "shop".into(),
                    ..Session::default()
                }],
                &HashSet::new()
            )
            .is_err()
        );
    }
}

#[test]
fn sidebar_failed_wake_attention_uses_exact_member_task_not_display_project() {
    let task = TaskKey {
        project: PROJECT.into(),
        task_id: "1d3fd318".into(),
    };
    let mut state = json!({"members":[
        {"session_id":DEVELOPER,"role":"developer","project":PROJECT,"task_id":"1d3fd318"},
        {"session_id":DESIGNER,"role":"designer","project":"path:/another/project-a","task_id":"1d3fd318"}],
        "sleeps":[{"session_id":DEVELOPER,"state":"failed","error":"launcher refused\u{1b}[2J"}]});
    let attention = sidebar::attention(&state, &task).expect("failed wake must remain visible");
    assert!(attention.contains("Developer") && attention.contains("launcher refused"));
    assert!(!attention.contains('\u{1b}'));
    assert!(
        sidebar::attention(
            &state,
            &TaskKey {
                project: "path:/another/project-a".into(),
                ..task.clone()
            }
        )
        .is_none()
    );
    assert!(
        sidebar::attention(
            &state,
            &TaskKey {
                task_id: "0123abcd".into(),
                ..task.clone()
            }
        )
        .is_none()
    );
    state["sleeps"][0]["state"] = json!("asleep");
    assert!(sidebar::attention(&state, &task).is_none());
    state["sleeps"][0]["state"] = json!("failed");
    state["sleeps"][0]["session_id"] = json!("unlinked-session");
    assert!(sidebar::attention(&state, &task).is_none());
}

fn link_snapshot() -> Value {
    json!({"link_proposals":[{"proposal_id":"lp1","sender":DESIGNER,"revision":2,"mapping_hash":"exact-hash",
        "task":{"project":"path:/first","task_id":"1234abcd"},"purpose":"Coordinate builds",
        "permission":"coordination_messages_only","status":"pending","blocked_reason":"TODO unreadable",
        "participants":[{"session_id":DESIGNER,"agent":"claude","project":"path:/first","runtime":"runtime-a"},
            {"session_id":DEVELOPER,"agent":"opencode","project":"path:/second","runtime":"runtime-b"}]}],
        "links":[{"link_id":"l1","proposal_id":"lp0","state":"closed","closure_reason":"done",
            "task":{"project":"path:/first","task_id":"1234abcd"}}]})
}

#[test]
fn link_requests_show_exact_provenance_permission_and_lifecycle_block() {
    let view = TeamView::new(link_snapshot());
    let detail = view.detail().join("\n");
    for expected in [
        DESIGNER,
        DEVELOPER,
        "path:/first",
        "path:/second",
        "runtime-a",
        "runtime-b",
        "1234abcd",
        "Coordinate builds",
        "Revision 2",
        "exact-hash",
        "messaging-only",
        "no team role",
        "TODO unreadable",
        "e edit",
        "x reject",
    ] {
        assert!(detail.contains(expected), "missing {expected:?}: {detail}");
    }
    assert_eq!(TeamView::attention(&link_snapshot()), 1);
}

#[test]
fn closed_links_remain_history_and_are_not_offered_for_revocation() {
    let mut view = TeamView::new(link_snapshot());
    view.is_history = true;
    let detail = view.detail().join("\n");
    assert!(detail.contains("l1") && detail.contains("done"), "{detail}");
    assert!(!detail.contains("u revoke"), "{detail}");
}

#[test]
fn live_link_identity_absence_or_replacement_is_visibly_blocked_not_closed() {
    for runtime in [None, Some("replacement-runtime")] {
        let mut state = link_snapshot();
        let mut link = state["link_proposals"][0].clone();
        link["state"] = json!("active");
        link["status"] = json!("approved");
        link["link_id"] = json!("active-link");
        link["blocked_reason"] = Value::Null;
        state["link_proposals"] = json!([]);
        state["links"] = json!([link]);
        state["sessions"] = if let Some(runtime) = runtime {
            json!([
            {"session_id":DESIGNER,"project":"path:/first","agent":"claude","runtime":runtime},
            {"session_id":DEVELOPER,"project":"path:/second","agent":"opencode","runtime":"runtime-b"}])
        } else {
            json!([])
        };
        assert_eq!(TeamView::attention(&state), 1);
        let detail = TeamView::new(state).detail().join("\n");
        assert!(
            detail.contains("Blocked") && detail.contains("active"),
            "{detail}"
        );
        assert!(
            detail.contains(if runtime.is_some() {
                "identity changed"
            } else {
                "absent"
            }),
            "{detail}"
        );
    }
}

#[test]
fn link_message_rows_distinguish_full_session_provenance_from_team_roles() {
    let mut state = snapshot();
    state["messages"][0]["envelope"] = json!(
        json!({"scope_kind":"link","link_id":"l1","kind":"notice","body":"Hello\u{1b}[2J",
        "sender":{"session_id":DESIGNER,"project":"path:/first","agent":"claude"},
        "recipient":{"session_id":DEVELOPER,"project":"path:/second","agent":"opencode"},
        "task":{"project":"path:/first","task_id":"1234abcd"}})
        .to_string()
    );
    let mut view = TeamView::new(state);
    view.selected = 3;
    let detail = view.detail().join("\n");
    for expected in [
        "link l1",
        DESIGNER,
        DEVELOPER,
        "path:/first",
        "path:/second",
        "1234abcd",
        "h it arrived",
        "c cancel",
    ] {
        assert!(detail.contains(expected), "missing {expected}: {detail}");
    }
    assert!(!detail.contains('\u{1b}') && !detail.contains("resend"));
    view.is_history = true;
    view.selected = 0;
    let history = view.detail().join("\n");
    for expected in [
        DESIGNER,
        DEVELOPER,
        "path:/second",
        "uncertain",
        "no receipt",
    ] {
        assert!(history.contains(expected), "missing {expected}: {history}");
    }
}
