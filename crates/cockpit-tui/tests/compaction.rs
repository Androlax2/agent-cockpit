//! A session's latest context compaction on its card (#782): the trigger, the sizes and the
//! seconds the harness reported, and nothing when it compacted nothing.

mod common;

use std::time::Instant;

use cockpit_protocol::{
    AgentState, Command, CompactionStatus, CompactionTrigger, CompactionView, Reply, SessionReport,
};
use cockpit_tui::App;
use common::{answer, app_with, screen_text, session};

const SESSION: &str = "s00000000000a";

fn report(compaction: Option<CompactionView>) -> SessionReport {
    SessionReport {
        compaction,
        session: SESSION.to_owned(),
        state: Some(AgentState::Working),
        label: None,
        doing: Some("reading the plan".to_owned()),
        needs_input: None,
        outcome: None,
        question_source: None,
        covered_question: None,
        held_by: None,
        reported_at: 5,
    }
}

fn compaction(status: CompactionStatus) -> CompactionView {
    CompactionView {
        status,
        trigger: Some(CompactionTrigger::Auto),
        tokens_before: Some(268_364),
        tokens_after: None,
        seconds: None,
        at: 5,
    }
}

fn app_reporting(report: SessionReport, now: Instant) -> App {
    let mut app = app_with(&["app"], vec![session(SESSION, 1, 1)], now);
    answer(
        &mut app,
        Command::AgentReports,
        Reply::AgentReports {
            reports: vec![report],
        },
        now,
    );
    app
}

#[test]
fn a_finished_compaction_shows_its_trigger_sizes_and_seconds() {
    let now = Instant::now();
    let finished = CompactionView {
        tokens_after: Some(30_470),
        seconds: Some(41),
        ..compaction(CompactionStatus::Completed)
    };
    let app = app_reporting(report(Some(finished)), now);

    let screen = screen_text(&app, now);

    assert!(
        screen.contains("compacted (auto): 268364 -> 30470 tokens in 41s"),
        "{screen}"
    );
}

#[test]
fn a_running_compaction_shows_the_size_it_started_from() {
    let now = Instant::now();
    let app = app_reporting(report(Some(compaction(CompactionStatus::Running))), now);

    let screen = screen_text(&app, now);

    assert!(
        screen.contains("compacting (auto) from 268364 tokens"),
        "{screen}"
    );
}

#[test]
fn a_failed_compaction_says_so() {
    let now = Instant::now();
    let app = app_reporting(report(Some(compaction(CompactionStatus::Failed))), now);

    let screen = screen_text(&app, now);

    assert!(screen.contains("compaction failed (auto)"), "{screen}");
}

#[test]
fn a_compaction_without_sizes_or_trigger_shows_only_its_status() {
    let now = Instant::now();
    let bare = CompactionView {
        trigger: None,
        tokens_before: None,
        ..compaction(CompactionStatus::Completed)
    };
    let app = app_reporting(report(Some(bare)), now);

    let screen = screen_text(&app, now);

    assert!(
        screen.contains("compacted\n") || screen.contains("compacted "),
        "{screen}"
    );
    assert!(!screen.contains("tokens"), "{screen}");
}

#[test]
fn a_session_that_never_compacted_shows_no_compaction_line() {
    let now = Instant::now();
    let app = app_reporting(report(None), now);

    let screen = screen_text(&app, now);

    assert!(!screen.contains("compact"), "{screen}");
}
