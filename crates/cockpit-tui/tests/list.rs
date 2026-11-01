//! The session list on a terminal too small for every card (#291), drawn with an injected
//! clock: the selected card stays whole on screen, and wrapped rows stay under the indent.

mod common;

use std::time::Instant;

use cockpit_protocol::{Command, Reply, SessionReport};
use cockpit_tui::{App, Key};
use common::{answer, app_with, press, session, sized_screen};

const LAST: &str = "s00000000000f";
const IDS: [&str; 6] = [
    "s00000000000a",
    "s00000000000b",
    "s00000000000c",
    "s00000000000d",
    "s00000000000e",
    LAST,
];

fn six_sessions(now: Instant) -> App {
    let sessions = IDS
        .iter()
        .zip(1..)
        .map(|(id, created_at)| session(id, 1, created_at))
        .collect();
    app_with(&["app"], sessions, now)
}

fn reported_doing(app: &mut App, session: &str, doing: &str, now: Instant) {
    answer(
        app,
        Command::AgentReports,
        Reply::AgentReports {
            reports: vec![SessionReport {
                session: session.to_owned(),
                state: None,
                label: None,
                doing: Some(doing.to_owned()),
                needs_input: None,
                outcome: None,
                question_source: None,
                covered_question: None,
                reported_at: 1,
            }],
        },
        now,
    );
}

/// The rows inside the list's border.
fn list_rows(screen: &[String]) -> Vec<&str> {
    screen
        .iter()
        .skip(1)
        .take_while(|row| !row.starts_with('└'))
        .map(String::as_str)
        .collect()
}

#[test]
fn selecting_the_last_card_scrolls_it_onto_the_screen() {
    let now = Instant::now();
    let mut app = six_sessions(now);

    press(&mut app, &[Key::Char('j'); 5], now);

    let screen = sized_screen(&app, now, 60, 6);
    assert!(
        list_rows(&screen)
            .iter()
            .any(|row| row.contains(&format!(">  {LAST}"))),
        "{screen:#?}"
    );
}

#[test]
fn the_first_card_shows_from_the_top_when_it_is_selected() {
    let now = Instant::now();
    let mut app = six_sessions(now);
    press(&mut app, &[Key::Char('j'); 5], now);

    press(&mut app, &[Key::Char('k'); 5], now);

    let screen = sized_screen(&app, now, 60, 6);
    assert!(list_rows(&screen)[0].starts_with("│app "), "{screen:#?}");
}

#[test]
fn a_selected_card_taller_than_one_row_shows_whole() {
    let now = Instant::now();
    let mut app = six_sessions(now);
    reported_doing(&mut app, LAST, "checking the cards", now);

    press(&mut app, &[Key::Char('j'); 5], now);

    let screen = sized_screen(&app, now, 60, 6);
    let rows = list_rows(&screen);
    assert!(
        rows.last()
            .is_some_and(|row| row.contains("checking the cards")),
        "{screen:#?}"
    );
}

#[test]
fn a_wrapped_details_line_continues_under_the_card_s_indent() {
    let now = Instant::now();
    let mut app = six_sessions(now);
    reported_doing(
        &mut app,
        IDS[0],
        "reading every report of the plan before it changes a line",
        now,
    );

    let screen = sized_screen(&app, now, 30, 20);

    let details: Vec<&str> = list_rows(&screen)
        .into_iter()
        .skip_while(|row| !row.contains("reading"))
        .take_while(|row| !row.contains(IDS[1]))
        .collect();
    assert!(details.len() > 1, "{screen:#?}");
    for row in details {
        assert!(
            row.starts_with("│   ") && !row.starts_with("│    "),
            "{row:?}"
        );
    }
}
