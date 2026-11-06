//! #62, #682: which event the notifier tells for what the daemon says, and when it tells
//! nothing. Driven through the change feed and the daemon's answers, as the runtime does.

mod common;

use cockpit_notify::{Event, Notice, Notifier};
use cockpit_protocol::{
    AgentExitView, AgentState, Command, EntityKind, ExitStatusView, QuestionSource, SessionReport,
    SessionStateView, SessionView, TurnOutcome,
};
use common::{
    after_snapshot, change_session, guide, loaded, network_request, report, run, session,
    set_network, set_queue, set_reports, tombstone, waiting_run,
};

const WORKER: &str = "s00000000000a";

fn worker() -> SessionView {
    session(WORKER, 1, 1)
}

fn running() -> Notifier {
    loaded(&[worker()], vec![report(WORKER)])
}

fn events(notices: &[Notice]) -> Vec<Event> {
    notices.iter().map(|notice| notice.event).collect()
}

fn asking(question: &str, source: QuestionSource) -> SessionReport {
    SessionReport {
        state: Some(AgentState::Waiting),
        needs_input: Some(question.to_owned()),
        question_source: Some(source),
        ..report(WORKER)
    }
}

fn ended_turn(outcome: TurnOutcome) -> SessionReport {
    SessionReport {
        state: Some(AgentState::Waiting),
        outcome: Some(outcome),
        ..report(WORKER)
    }
}

#[test]
fn a_session_the_task_list_says_finished_is_told_once() {
    let mut notifier = running();
    let finished = SessionView {
        finished: true,
        ..worker()
    };

    assert_eq!(
        events(&change_session(&mut notifier, &finished)),
        [Event::Finished]
    );
    assert_eq!(change_session(&mut notifier, &finished), []);
}

#[test]
fn a_finish_the_feed_and_the_report_both_show_is_told_once() {
    let mut notifier = running();

    let from_feed = change_session(
        &mut notifier,
        &SessionView {
            finished: true,
            ..worker()
        },
    );
    let from_report = set_reports(&mut notifier, vec![ended_turn(TurnOutcome::Completed)]);

    assert_eq!(events(&from_feed), [Event::Finished]);
    assert_eq!(from_report, []);
}

#[test]
fn each_turn_outcome_is_its_own_event() {
    for (outcome, event) in [
        (TurnOutcome::Completed, Event::Finished),
        (TurnOutcome::Failed { reason: None }, Event::Failure),
        (TurnOutcome::QuotaStopped, Event::Quota),
    ] {
        let mut notifier = running();

        let told = set_reports(&mut notifier, vec![ended_turn(outcome.clone())]);

        assert_eq!(events(&told), [event], "{outcome:?}");
    }
}

#[test]
fn the_agents_own_question_is_a_question_with_its_text() {
    let mut notifier = running();

    let told = set_reports(
        &mut notifier,
        vec![asking("Which branch?", QuestionSource::Agent)],
    );

    assert_eq!(events(&told), [Event::Question]);
    assert_eq!(told[0].detail.as_deref(), Some("Which branch?"));
}

#[test]
fn a_providers_permission_dialog_is_a_hold() {
    let mut notifier = running();

    let told = set_reports(
        &mut notifier,
        vec![asking("Allow Bash(rm -rf build)?", QuestionSource::Dialog)],
    );

    assert_eq!(events(&told), [Event::Hold]);
}

#[test]
fn a_question_a_coordinator_holds_is_not_told() {
    let mut notifier = running();

    let told = set_reports(
        &mut notifier,
        vec![SessionReport {
            held_by: Some("s00000000000c".to_owned()),
            ..asking("Which branch?", QuestionSource::Agent)
        }],
    );

    assert_eq!(told, []);
}

#[test]
fn a_command_waiting_for_approval_is_a_hold_naming_the_command() {
    let mut notifier = running();

    let told = set_queue(&mut notifier, vec![run(1, WORKER, "make test")], Vec::new());

    assert_eq!(events(&told), [Event::Hold]);
    assert_eq!(told[0].detail.as_deref(), Some("make test"));
}

#[test]
fn a_refused_network_host_is_a_hold_naming_the_host() {
    let mut notifier = running();

    let told = set_network(&mut notifier, vec![network_request(4, WORKER)]);

    assert_eq!(events(&told), [Event::Hold]);
    assert_eq!(told[0].detail.as_deref(), Some("unknown.example.com:443"));
}

#[test]
fn a_guide_or_a_command_waiting_for_input_is_a_question() {
    let mut notifier = running();

    let told = set_queue(
        &mut notifier,
        vec![waiting_run(2, WORKER)],
        vec![guide(3, WORKER, None)],
    );

    assert_eq!(events(&told), [Event::Question, Event::Question]);
}

#[test]
fn a_second_approval_while_the_first_is_open_is_told_again() {
    let mut notifier = running();
    set_queue(&mut notifier, vec![run(1, WORKER, "make test")], Vec::new());

    let told = set_queue(
        &mut notifier,
        vec![run(1, WORKER, "make test"), run(2, WORKER, "make lint")],
        Vec::new(),
    );

    assert_eq!(told.len(), 1);
    assert_eq!(told[0].detail.as_deref(), Some("make lint"));
}

#[test]
fn a_session_that_failed_is_told_with_its_reason() {
    let mut notifier = running();

    let told = change_session(
        &mut notifier,
        &SessionView {
            state: SessionStateView::Failed,
            reason: Some("the launch failed".to_owned()),
            ..worker()
        },
    );

    assert_eq!(events(&told), [Event::Failure]);
    assert_eq!(told[0].detail.as_deref(), Some("the launch failed"));
}

#[test]
fn an_agent_that_exited_by_itself_is_a_failure() {
    let mut notifier = running();

    let told = change_session(
        &mut notifier,
        &SessionView {
            state: SessionStateView::Ended,
            exit: Some(AgentExitView {
                status: ExitStatusView::Code { code: 1 },
                output: vec!["panic".to_owned()],
            }),
            ..worker()
        },
    );

    assert_eq!(events(&told), [Event::Failure]);
}

#[test]
fn a_session_the_owner_ended_is_not_told() {
    let mut notifier = running();

    let told = change_session(
        &mut notifier,
        &SessionView {
            state: SessionStateView::Ended,
            ..worker()
        },
    );

    assert_eq!(told, []);
}

#[test]
fn a_session_taken_off_the_feed_is_not_told() {
    let mut notifier = running();

    let told = notifier
        .apply_changes(&after_snapshot(vec![tombstone(
            EntityKind::Session,
            WORKER,
        )]))
        .unwrap();

    assert_eq!(told, []);
}

#[test]
fn events_that_land_together_come_in_one_answer() {
    let mut notifier = running();

    let told = set_queue(
        &mut notifier,
        vec![run(1, WORKER, "make test")],
        vec![guide(2, WORKER, None)],
    );

    assert_eq!(events(&told), [Event::Hold, Event::Question]);
}

#[test]
fn what_already_needed_the_owner_at_start_is_not_told() {
    let mut notifier = loaded(
        &[SessionView {
            finished: true,
            ..worker()
        }],
        vec![asking("Which branch?", QuestionSource::Agent)],
    );

    let refreshed = set_reports(
        &mut notifier,
        vec![asking("Which branch?", QuestionSource::Agent)],
    );

    assert_eq!(refreshed, []);
}

#[test]
fn a_report_request_that_failed_does_not_keep_the_notifier_silent() {
    let mut notifier = Notifier::default();
    notifier
        .apply_changes(&common::snapshot(vec![common::upsert(
            EntityKind::Session,
            WORKER,
            &worker(),
        )]))
        .unwrap();
    notifier.answer(&Command::AgentReports, Err("timed out".to_owned()));
    set_queue(&mut notifier, Vec::new(), Vec::new());
    set_network(&mut notifier, Vec::new());

    let told = set_reports(
        &mut notifier,
        vec![asking("Which branch?", QuestionSource::Agent)],
    );

    assert_eq!(events(&told), [Event::Question]);
}

#[test]
fn a_notice_names_the_session_by_its_reported_label() {
    let mut notifier = running();

    let told = set_reports(
        &mut notifier,
        vec![SessionReport {
            label: Some("fix login".to_owned()),
            ..ended_turn(TurnOutcome::Completed)
        }],
    );

    assert_eq!(told[0].name, "fix login");
}
