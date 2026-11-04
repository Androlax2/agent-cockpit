//! #682: the TUI rings the terminal bell once when a session needs the owner: it finished,
//! it asks a question or waits for an approval, or it failed or ended on its own. The tests
//! drive the app through the change feed and the daemon's answers, as the runtime does, and
//! ask it for the bell after each.

mod common;

use std::time::Instant;

use cockpit_protocol::{
    AgentExitView, AgentState, Command, EntityKind, ExitStatusView, QuestionSource, Reply,
    SessionReport, SessionStateView, SessionView, TurnOutcome,
};
use cockpit_tui::App;
use common::{after_snapshot, answer, app_with, queue, run, session, tombstone, upsert};

const WORKER: &str = "s00000000000a";

fn report(session: &str) -> SessionReport {
    SessionReport {
        session: session.to_owned(),
        state: Some(AgentState::Working),
        label: None,
        doing: None,
        needs_input: None,
        outcome: None,
        question_source: None,
        covered_question: None,
        reported_at: 1,
    }
}

fn asking(session: &str) -> SessionReport {
    SessionReport {
        state: Some(AgentState::Waiting),
        needs_input: Some("Which branch?".to_owned()),
        question_source: Some(QuestionSource::Agent),
        ..report(session)
    }
}

fn completed(session: &str) -> SessionReport {
    SessionReport {
        state: Some(AgentState::Waiting),
        outcome: Some(TurnOutcome::Completed),
        ..report(session)
    }
}

fn reports(app: &mut App, reports: Vec<SessionReport>, now: Instant) {
    answer(
        app,
        Command::AgentReports,
        Reply::AgentReports { reports },
        now,
    );
}

fn change_session(app: &mut App, view: &SessionView) {
    app.apply_changes(&after_snapshot(vec![upsert(
        EntityKind::Session,
        &view.id,
        view,
    )]))
    .expect("the change applies");
}

/// An app that has loaded everything the daemon tells it at start, with one working session
/// and nothing that needs the owner. The bell is quiet until then.
fn running_app(now: Instant) -> App {
    let mut app = app_with(&["app"], vec![session(WORKER, 1, 1)], now);
    reports(&mut app, vec![report(WORKER)], now);
    queue(&mut app, Vec::new(), Vec::new(), now);
    assert!(!app.take_bell(), "loading the start state rings nothing");
    app
}

#[test]
fn a_session_the_task_list_says_finished_rings_once() {
    let now = Instant::now();
    let mut app = running_app(now);

    change_session(
        &mut app,
        &SessionView {
            finished: true,
            ..session(WORKER, 1, 1)
        },
    );

    assert!(app.take_bell());
    assert!(!app.take_bell(), "one cue, one bell");
}

#[test]
fn a_turn_that_completed_rings_once() {
    let now = Instant::now();
    let mut app = running_app(now);

    reports(&mut app, vec![completed(WORKER)], now);

    assert!(app.take_bell());
    assert!(!app.take_bell());
}

#[test]
fn the_finish_that_the_feed_and_the_report_both_show_rings_once() {
    let now = Instant::now();
    let mut app = running_app(now);

    change_session(
        &mut app,
        &SessionView {
            finished: true,
            ..session(WORKER, 1, 1)
        },
    );
    reports(&mut app, vec![completed(WORKER)], now);

    assert!(app.take_bell());
    assert!(!app.take_bell());
}

#[test]
fn a_question_the_agent_asks_rings_once() {
    let now = Instant::now();
    let mut app = running_app(now);

    reports(&mut app, vec![asking(WORKER)], now);

    assert!(app.take_bell());
    assert!(!app.take_bell());
}

#[test]
fn an_approval_waiting_for_the_owner_rings_once() {
    let now = Instant::now();
    let mut app = running_app(now);

    queue(&mut app, vec![run(1, WORKER, "make test")], Vec::new(), now);

    assert!(app.take_bell());
    assert!(!app.take_bell());
}

#[test]
fn a_session_that_failed_rings_once() {
    let now = Instant::now();
    let mut app = running_app(now);

    change_session(
        &mut app,
        &SessionView {
            state: SessionStateView::Failed,
            reason: Some("the launch failed".to_owned()),
            ..session(WORKER, 1, 1)
        },
    );

    assert!(app.take_bell());
    assert!(!app.take_bell());
}

#[test]
fn a_turn_that_failed_rings_once() {
    let now = Instant::now();
    let mut app = running_app(now);

    reports(
        &mut app,
        vec![SessionReport {
            outcome: Some(TurnOutcome::Failed { reason: None }),
            ..report(WORKER)
        }],
        now,
    );

    assert!(app.take_bell());
}

#[test]
fn an_agent_that_exited_by_itself_rings_once() {
    let now = Instant::now();
    let mut app = running_app(now);

    change_session(
        &mut app,
        &SessionView {
            state: SessionStateView::Ended,
            exit: Some(AgentExitView {
                status: ExitStatusView::Code { code: 1 },
                output: vec!["panic".to_owned()],
            }),
            ..session(WORKER, 1, 1)
        },
    );

    assert!(app.take_bell());
    assert!(!app.take_bell());
}

#[test]
fn a_session_the_owner_ended_rings_nothing() {
    let now = Instant::now();
    let mut app = running_app(now);

    change_session(
        &mut app,
        &SessionView {
            state: SessionStateView::Ended,
            ..session(WORKER, 1, 1)
        },
    );

    assert!(!app.take_bell());
}

#[test]
fn a_session_taken_off_the_feed_rings_nothing() {
    let now = Instant::now();
    let mut app = running_app(now);

    app.apply_changes(&after_snapshot(vec![tombstone(
        EntityKind::Session,
        WORKER,
    )]))
    .expect("the change applies");

    assert!(!app.take_bell());
}

#[test]
fn several_cues_that_land_together_ring_once() {
    let now = Instant::now();
    let mut app = app_with(
        &["app"],
        vec![session(WORKER, 1, 1), session("s00000000000b", 1, 2)],
        now,
    );
    reports(&mut app, vec![report(WORKER), report("s00000000000b")], now);
    queue(&mut app, Vec::new(), Vec::new(), now);
    app.take_bell();

    reports(
        &mut app,
        vec![asking(WORKER), completed("s00000000000b")],
        now,
    );

    assert!(app.take_bell());
    assert!(!app.take_bell());
}

#[test]
fn the_same_state_shown_again_by_the_next_refresh_rings_nothing() {
    let now = Instant::now();
    let mut app = running_app(now);
    reports(&mut app, vec![asking(WORKER)], now);
    app.take_bell();

    reports(&mut app, vec![asking(WORKER)], now);
    queue(&mut app, Vec::new(), Vec::new(), now);

    assert!(!app.take_bell());
}

#[test]
fn answering_the_question_rings_nothing() {
    let now = Instant::now();
    let mut app = running_app(now);
    reports(&mut app, vec![asking(WORKER)], now);
    app.take_bell();

    reports(&mut app, vec![report(WORKER)], now);

    assert!(!app.take_bell());
}

#[test]
fn a_second_question_after_the_first_was_answered_rings_again() {
    let now = Instant::now();
    let mut app = running_app(now);
    reports(&mut app, vec![asking(WORKER)], now);
    app.take_bell();
    reports(&mut app, vec![report(WORKER)], now);

    reports(&mut app, vec![asking(WORKER)], now);

    assert!(app.take_bell());
}

#[test]
fn what_already_needed_the_owner_when_the_tui_started_rings_nothing() {
    let now = Instant::now();
    let mut app = app_with(
        &["app"],
        vec![SessionView {
            finished: true,
            ..session(WORKER, 1, 1)
        }],
        now,
    );

    reports(&mut app, vec![asking(WORKER)], now);
    queue(&mut app, vec![run(1, WORKER, "make test")], Vec::new(), now);

    assert!(!app.take_bell());
}

#[test]
fn a_muted_bell_never_rings() {
    let now = Instant::now();
    let mut app = running_app(now).with_attention_bell(false);

    reports(&mut app, vec![asking(WORKER)], now);

    assert!(!app.take_bell());
}

#[test]
fn a_cue_that_landed_while_muted_does_not_ring_when_the_bell_is_on_again() {
    let now = Instant::now();
    let mut app = running_app(now).with_attention_bell(false);
    reports(&mut app, vec![asking(WORKER)], now);
    app.take_bell();

    let mut app = app.with_attention_bell(true);

    assert!(!app.take_bell());
}

#[test]
fn a_second_approval_while_the_first_is_still_open_rings_again() {
    let now = Instant::now();
    let mut app = running_app(now);
    queue(&mut app, vec![run(1, WORKER, "make test")], Vec::new(), now);
    app.take_bell();

    queue(
        &mut app,
        vec![run(1, WORKER, "make test"), run(2, WORKER, "make lint")],
        Vec::new(),
        now,
    );

    assert!(app.take_bell());
}

#[test]
fn a_new_question_while_the_old_one_is_still_open_rings_again() {
    let now = Instant::now();
    let mut app = running_app(now);
    reports(&mut app, vec![asking(WORKER)], now);
    app.take_bell();

    reports(
        &mut app,
        vec![SessionReport {
            needs_input: Some("And which remote?".to_owned()),
            ..asking(WORKER)
        }],
        now,
    );

    assert!(app.take_bell());
}

#[test]
fn a_report_request_that_failed_does_not_keep_the_bell_silent() {
    let now = Instant::now();
    let mut app = app_with(&["app"], vec![session(WORKER, 1, 1)], now);
    queue(&mut app, Vec::new(), Vec::new(), now);
    app.answer(&Command::AgentReports, Err("timed out".to_owned()), now);
    app.take_bell();

    reports(&mut app, vec![asking(WORKER)], now);

    assert!(app.take_bell());
}
