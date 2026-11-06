//! Issue #710: a question the coordinator that started a session holds stays off the owner's
//! needs-you list; the card names the coordinator instead.

mod common;

use std::time::Instant;

use cockpit_protocol::{AgentState, Command, QuestionSource, Reply, SessionReport, SessionView};
use common::{answer, app_with, screen_text, session};

const WORKER: &str = "s00000000000b";
const COORDINATOR: &str = "s00000000000a";

fn asked(held_by: Option<&str>) -> SessionReport {
    SessionReport {
        compaction: None,
        session: WORKER.to_owned(),
        state: Some(AgentState::Waiting),
        label: None,
        doing: None,
        needs_input: Some("Which branch?".to_owned()),
        outcome: None,
        question_source: Some(QuestionSource::Agent),
        covered_question: None,
        held_by: held_by.map(str::to_owned),
        reported_at: 1,
    }
}

fn screen_with(report: SessionReport) -> String {
    screen_of(session(WORKER, 1, 1), report)
}

fn screen_of(worker: SessionView, report: SessionReport) -> String {
    let now = Instant::now();
    let mut app = app_with(&["app"], vec![worker], now);
    answer(
        &mut app,
        Command::AgentReports,
        Reply::AgentReports {
            reports: vec![report],
        },
        now,
    );
    screen_text(&app, now)
}

#[test]
fn a_question_its_coordinator_holds_names_the_coordinator_instead_of_waiting_on_you() {
    let screen = screen_with(asked(Some(COORDINATOR)));

    assert!(
        screen.contains(&format!("question with coordinator {COORDINATOR}")),
        "{screen}"
    );
    assert!(!screen.contains("waiting on you"), "{screen}");
    assert!(!screen.contains(&format!("! {WORKER}")), "{screen}");
}

#[test]
fn a_question_no_coordinator_holds_waits_on_you() {
    let screen = screen_with(asked(None));

    assert!(screen.contains("waiting on you"), "{screen}");
    assert!(!screen.contains("question with coordinator"), "{screen}");
}

#[test]
fn a_finished_session_whose_question_its_coordinator_holds_stays_listed() {
    let finished = SessionView {
        finished: true,
        ..session(WORKER, 1, 1)
    };

    let screen = screen_of(finished, asked(Some(COORDINATOR)));

    assert!(
        screen.contains(&format!("question with coordinator {COORDINATOR}")),
        "{screen}"
    );
    assert!(!screen.contains("Finished (1)"), "{screen}");
}
