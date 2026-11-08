//! A session whose process runs but whose agent waits with nothing in progress shows as
//! waiting, not running (#864); a resumed one says it is idle until it is messaged.

mod common;

use std::time::Instant;

use cockpit_protocol::{
    AgentState, Command, LaunchKindView, LaunchView, Reply, SessionReport, SessionStateView,
};
use cockpit_tui::App;
use common::{answer, app_with, screen_text, session};

const SESSION: &str = "s00000000000a";
const LAUNCHED_AT: i64 = 100;

fn report(state: Option<AgentState>, doing: Option<&str>, reported_at: i64) -> SessionReport {
    SessionReport {
        compaction: None,
        session: SESSION.to_owned(),
        state,
        label: None,
        doing: doing.map(str::to_owned),
        needs_input: None,
        outcome: None,
        question_source: None,
        covered_question: None,
        held_by: None,
        reported_at,
    }
}

fn app_after_launch(kind: LaunchKindView, reports: Vec<SessionReport>, now: Instant) -> App {
    let mut launched = session(SESSION, 1, 1);
    launched.launch = Some(LaunchView {
        number: 1,
        kind,
        launched_at: LAUNCHED_AT,
        account: None,
    });
    let mut app = app_with(&["app"], vec![launched], now);
    answer(
        &mut app,
        Command::AgentReports,
        Reply::AgentReports { reports },
        now,
    );
    app
}

#[test]
fn a_running_session_whose_agent_waits_with_nothing_in_progress_shows_waiting() {
    let now = Instant::now();
    let reports = vec![report(Some(AgentState::Waiting), None, LAUNCHED_AT + 5)];
    let app = app_after_launch(LaunchKindView::Start, reports, now);

    let screen = screen_text(&app, now);

    assert!(screen.contains("fake  waiting (running)"), "{screen}");
}

#[test]
fn a_running_session_whose_agent_works_shows_running() {
    let now = Instant::now();
    let reports = vec![report(
        Some(AgentState::Working),
        Some("reading the plan"),
        LAUNCHED_AT + 5,
    )];
    let app = app_after_launch(LaunchKindView::Start, reports, now);

    let screen = screen_text(&app, now);

    assert!(screen.contains("fake  running"), "{screen}");
    assert!(!screen.contains("waiting"), "{screen}");
}

#[test]
fn a_waiting_agent_with_something_in_progress_still_shows_running() {
    let now = Instant::now();
    let reports = vec![report(
        Some(AgentState::Waiting),
        Some("watching CI"),
        LAUNCHED_AT + 5,
    )];
    let app = app_after_launch(LaunchKindView::Start, reports, now);

    let screen = screen_text(&app, now);

    assert!(screen.contains("fake  running"), "{screen}");
}

#[test]
fn a_session_without_a_report_shows_its_lifecycle() {
    let now = Instant::now();
    let app = app_after_launch(LaunchKindView::Start, Vec::new(), now);

    let screen = screen_text(&app, now);

    assert!(screen.contains("fake  running"), "{screen}");
}

#[test]
fn a_session_that_is_not_running_keeps_its_lifecycle_word() {
    let now = Instant::now();
    let mut asleep = session(SESSION, 1, 1);
    asleep.state = SessionStateView::Asleep;
    let mut app = app_with(&["app"], vec![asleep], now);
    answer(
        &mut app,
        Command::AgentReports,
        Reply::AgentReports {
            reports: vec![report(Some(AgentState::Waiting), None, 5)],
        },
        now,
    );

    let screen = screen_text(&app, now);

    assert!(screen.contains("fake  asleep"), "{screen}");
}

#[test]
fn a_resumed_session_says_it_is_idle_until_it_is_messaged() {
    let now = Instant::now();
    let app = app_after_launch(LaunchKindView::Resume, Vec::new(), now);

    let screen = screen_text(&app, now);

    assert!(screen.contains("fake  waiting (running)"), "{screen}");
    assert!(
        screen.contains("resumed, idle until you send it something"),
        "{screen}"
    );
}

#[test]
fn a_report_from_before_the_resume_does_not_make_a_resumed_session_look_busy() {
    let now = Instant::now();
    let reports = vec![report(Some(AgentState::Working), None, LAUNCHED_AT - 50)];
    let app = app_after_launch(LaunchKindView::Resume, reports, now);

    let screen = screen_text(&app, now);

    assert!(screen.contains("fake  waiting (running)"), "{screen}");
    assert!(
        screen.contains("resumed, idle until you send it something"),
        "{screen}"
    );
}

#[test]
fn a_resumed_session_that_works_again_shows_running() {
    let now = Instant::now();
    let reports = vec![report(
        Some(AgentState::Working),
        Some("answering the owner"),
        LAUNCHED_AT + 30,
    )];
    let app = app_after_launch(LaunchKindView::Resume, reports, now);

    let screen = screen_text(&app, now);

    assert!(screen.contains("fake  running"), "{screen}");
    assert!(!screen.contains("resumed, idle"), "{screen}");
}

#[test]
fn a_resumed_session_that_finished_its_turn_shows_waiting_without_the_resume_note() {
    let now = Instant::now();
    let reports = vec![report(Some(AgentState::Waiting), None, LAUNCHED_AT + 30)];
    let app = app_after_launch(LaunchKindView::Resume, reports, now);

    let screen = screen_text(&app, now);

    assert!(screen.contains("fake  waiting (running)"), "{screen}");
    assert!(!screen.contains("resumed, idle"), "{screen}");
}

#[test]
fn a_resumed_session_does_not_show_what_it_did_before_the_resume() {
    let now = Instant::now();
    let reports = vec![report(
        Some(AgentState::Working),
        Some("reading the plan"),
        LAUNCHED_AT - 50,
    )];
    let app = app_after_launch(LaunchKindView::Resume, reports, now);

    let screen = screen_text(&app, now);

    assert!(!screen.contains("reading the plan"), "{screen}");
}

#[test]
fn a_waiting_agent_that_asked_the_owner_keeps_its_lifecycle_word() {
    let now = Instant::now();
    let mut asking = report(Some(AgentState::Waiting), None, LAUNCHED_AT + 5);
    asking.needs_input = Some("which branch?".to_owned());
    asking.question_source = Some(cockpit_protocol::QuestionSource::Agent);
    let app = app_after_launch(LaunchKindView::Start, vec![asking], now);

    let screen = screen_text(&app, now);

    assert!(screen.contains("fake  running"), "{screen}");
    assert!(screen.contains("waiting on you"), "{screen}");
}
