//! A failed session's card says how its agent exited and the last line it printed (#632).

mod common;

use std::time::Instant;

use cockpit_protocol::{AgentExitView, ExitStatusView, SessionStateView};
use common::{app_with, screen_text, session};

fn refused(state: SessionStateView) -> cockpit_protocol::SessionView {
    let mut refused = session("s00000000000a", 1, 1);
    refused.state = state;
    refused.exit = Some(AgentExitView {
        status: ExitStatusView::Code { code: 3 },
        output: vec![
            "starting".to_owned(),
            "Error: failed to wrap command: refusing to start".to_owned(),
        ],
    });
    refused
}

#[test]
fn a_failed_card_shows_the_exit_status_and_the_last_output_line() {
    let now = Instant::now();
    let app = app_with(&["app"], vec![refused(SessionStateView::Failed)], now);

    let screen = screen_text(&app, now);

    assert!(
        screen.contains("exited with status 3 · Error: failed to wrap command: refusing to start"),
        "{screen}"
    );
}

#[test]
fn a_running_card_has_no_exit_line() {
    let now = Instant::now();
    let app = app_with(&["app"], vec![refused(SessionStateView::Running)], now);

    let screen = screen_text(&app, now);

    assert!(!screen.contains("exited with"), "{screen}");
}

#[test]
fn a_long_last_line_is_cut_on_the_card() {
    let now = Instant::now();
    let mut failed = refused(SessionStateView::Failed);
    let long_line = format!("{} TAIL", "word ".repeat(60));
    failed.exit = Some(AgentExitView {
        status: ExitStatusView::Code { code: 3 },
        output: vec![long_line],
    });
    let app = app_with(&["app"], vec![failed], now);

    let screen = screen_text(&app, now);

    assert!(screen.contains('…'), "{screen}");
    assert!(!screen.contains("TAIL"), "{screen}");
}
