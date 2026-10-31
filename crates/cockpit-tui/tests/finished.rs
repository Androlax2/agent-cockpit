//! #157 (decided in #201): sessions the task list says finished fold into one group under
//! the projects, out of the selectable list, and `X` exits them together after `y`.

mod common;

use std::time::Instant;

use cockpit_protocol::{Command, Reply, SessionStateView, SessionView};
use cockpit_tui::Key;
use common::{answer, app_with, press, queue, run, screen_text, sent, session};

fn finished(id: &str, created_at: i64) -> SessionView {
    SessionView {
        finished: true,
        ..session(id, 1, created_at)
    }
}

fn one_working_two_finished(now: Instant) -> cockpit_tui::App {
    app_with(
        &["app"],
        vec![
            session("s00000000000a", 1, 1),
            finished("s00000000000b", 2),
            finished("s00000000000c", 3),
        ],
        now,
    )
}

#[test]
fn finished_sessions_fold_into_one_group_and_leave_the_project_s_list() {
    let now = Instant::now();
    let app = one_working_two_finished(now);

    let shown = screen_text(&app, now);

    assert!(
        shown.contains("Finished (2), folded: X exits them"),
        "{shown}"
    );
    assert!(!shown.contains("s00000000000b"), "{shown}");
    assert!(shown.contains("s00000000000a"), "{shown}");
}

#[test]
fn moving_through_the_list_skips_the_folded_finished_sessions() {
    let now = Instant::now();
    let mut app = one_working_two_finished(now);

    press(&mut app, &[Key::Char('j'), Key::Char('j')], now);

    assert_eq!(app.selected(), Some("s00000000000a"));
}

#[test]
fn x_upper_case_exits_every_finished_session_only_after_y() {
    let now = Instant::now();
    let mut app = one_working_two_finished(now);

    let asked = app.key(Key::Char('X'), now);
    let shown = screen_text(&app, now);
    let confirmed = app.key(Key::Char('y'), now);

    assert_eq!(asked.used, Some("exit-finished-ask"));
    assert!(sent(&[asked]).is_empty());
    assert!(
        shown.contains("exit 2 finished sessions? y to confirm"),
        "{shown}"
    );
    assert_eq!(
        confirmed.send,
        [Command::SessionEndFinished {
            only: Some(vec!["s00000000000b".to_owned(), "s00000000000c".to_owned()])
        }]
    );
}

#[test]
fn x_upper_case_asks_nothing_when_no_session_is_finished() {
    let now = Instant::now();
    let mut app = app_with(&["app"], vec![session("s00000000000a", 1, 1)], now);

    let steps = press(&mut app, &[Key::Char('X'), Key::Char('y')], now);

    assert!(sent(&steps).is_empty());
}

#[test]
fn the_ended_finished_sessions_leave_the_group() {
    let now = Instant::now();
    let mut app = one_working_two_finished(now);
    let ended = |id: &str, created_at| SessionView {
        state: SessionStateView::Ended,
        ..session(id, 1, created_at)
    };

    answer(
        &mut app,
        Command::SessionEndFinished { only: None },
        Reply::EndedFinished {
            ended: vec![ended("s00000000000b", 2), ended("s00000000000c", 3)],
            failed: Vec::new(),
        },
        now,
    );

    let shown = screen_text(&app, now);
    assert!(!shown.contains("Finished"), "{shown}");
}

#[test]
fn a_finished_session_asking_for_an_approval_stays_listed() {
    let now = Instant::now();
    let mut app = one_working_two_finished(now);

    queue(
        &mut app,
        vec![run(7, "s00000000000b", "make test")],
        Vec::new(),
        now,
    );

    let shown = screen_text(&app, now);
    assert!(shown.contains("Finished (1), folded"), "{shown}");
    assert!(
        app.board()
            .listed_sessions()
            .iter()
            .any(|listed| listed.id == "s00000000000b")
    );
}
