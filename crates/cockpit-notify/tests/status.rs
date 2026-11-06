//! #62, report 19: what the status file for bars shows, from what the daemon says, and the
//! file's schema as a bar reads it.

mod common;

use cockpit_notify::{Notifier, Status, write_status};
use cockpit_protocol::{
    AgentExitView, AgentState, EntityKind, ExitStatusView, QuestionSource, SessionReport,
    SessionStateView, SessionView,
};
use common::{
    after_snapshot, change_session, checkout, loaded, report, session, set_reports, upsert,
};

const WORKER: &str = "s00000000000a";

fn worker() -> SessionView {
    session(WORKER, 1, 1)
}

fn status_of(notifier: &Notifier) -> Status {
    notifier.status(true)
}

#[test]
fn a_working_session_shows_its_label_project_and_doing() {
    let mut notifier = loaded(
        &[worker()],
        vec![SessionReport {
            label: Some("api".to_owned()),
            doing: Some("writing tests".to_owned()),
            reported_at: 50,
            ..report(WORKER)
        }],
    );
    notifier
        .apply_changes(&after_snapshot(vec![upsert(
            EntityKind::Checkout,
            "1",
            &checkout(1, "agent-cockpit"),
        )]))
        .unwrap();

    let status = status_of(&notifier);

    assert!(status.reachable);
    assert_eq!(status.needs_you, 0);
    let shown = &status.sessions[0];
    assert_eq!(shown.name, "api");
    assert_eq!(shown.project.as_deref(), Some("agent-cockpit"));
    assert_eq!(shown.state, "working");
    assert_eq!(shown.doing.as_deref(), Some("writing tests"));
    assert_eq!(shown.changed_at, 50);
}

#[test]
fn a_question_puts_the_session_in_needs_you_and_counts_it() {
    let mut notifier = loaded(&[worker()], vec![report(WORKER)]);
    set_reports(
        &mut notifier,
        vec![SessionReport {
            state: Some(AgentState::Waiting),
            needs_input: Some("Which branch?".to_owned()),
            question_source: Some(QuestionSource::Agent),
            ..report(WORKER)
        }],
    );

    let status = status_of(&notifier);

    assert_eq!(status.needs_you, 1);
    assert_eq!(status.sessions[0].state, "needs-you");
    assert_eq!(status.sessions[0].needs, ["question"]);
}

#[test]
fn a_finished_session_keeps_its_state_and_lists_finished() {
    let mut notifier = loaded(&[worker()], vec![report(WORKER)]);
    set_reports(
        &mut notifier,
        vec![SessionReport {
            state: Some(AgentState::Waiting),
            ..report(WORKER)
        }],
    );
    change_session(
        &mut notifier,
        &SessionView {
            finished: true,
            ..worker()
        },
    );

    let status = status_of(&notifier);

    assert_eq!(status.needs_you, 0);
    assert_eq!(status.sessions[0].state, "waiting");
    assert_eq!(status.sessions[0].needs, ["finished"]);
}

#[test]
fn an_ended_session_that_needs_nothing_is_left_out() {
    let mut notifier = loaded(&[worker()], vec![report(WORKER)]);
    change_session(
        &mut notifier,
        &SessionView {
            state: SessionStateView::Ended,
            ..worker()
        },
    );

    assert_eq!(status_of(&notifier).sessions, []);
}

#[test]
fn a_session_whose_agent_exited_by_itself_needs_the_owner_for_its_failure() {
    let mut notifier = loaded(&[worker()], vec![report(WORKER)]);
    change_session(
        &mut notifier,
        &SessionView {
            state: SessionStateView::Ended,
            exit: Some(AgentExitView {
                status: ExitStatusView::Code { code: 1 },
                output: Vec::new(),
            }),
            ..worker()
        },
    );

    let status = status_of(&notifier);

    assert_eq!(status.needs_you, 1);
    assert_eq!(status.sessions[0].state, "needs-you");
    assert_eq!(status.sessions[0].needs, ["failure"]);
}

#[test]
fn a_daemon_out_of_reach_or_not_yet_loaded_is_not_reachable() {
    assert!(!Notifier::default().status(true).reachable);
    assert!(
        !loaded(&[worker()], vec![report(WORKER)])
            .status(false)
            .reachable
    );
}

#[test]
fn the_file_carries_its_schema_and_when_it_was_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("status.json");
    let notifier = loaded(&[worker()], vec![report(WORKER)]);

    write_status(&path, &status_of(&notifier), 1_700_000_000).unwrap();

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        written,
        serde_json::json!({
            "schema": 1,
            "written_at": 1_700_000_000,
            "reachable": true,
            "needs_you": 0,
            "sessions": [{
                "id": WORKER,
                "name": WORKER,
                "project": null,
                "state": "working",
                "needs": [],
                "doing": null,
                "changed_at": 1,
            }],
        })
    );
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["status.json"], "no partial file is left");
}
