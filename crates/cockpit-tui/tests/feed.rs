//! The session list rendered from a recorded change feed (#35): grouped by project, every
//! batch applied whole or refused whole, and "waiting on you" on a card without freezing.

mod common;

use std::time::Instant;

use cockpit_protocol::{ChangeBatch, Command, EntityKind, FeedPosition, Reply};
use cockpit_tui::{App, Key};
use common::{answer, project, queue, run, screen, screen_text, session, upsert};

/// The batches of `tests/fixtures/two-projects.feed`, as the daemon sent them.
fn recorded_feed() -> Vec<ChangeBatch> {
    include_str!("fixtures/two-projects.feed")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a recorded change batch"))
        .collect()
}

fn app_on_projects() -> App {
    let mut app = App::new();
    answer(
        &mut app,
        Command::ProjectList,
        Reply::Projects {
            projects: vec![project("web"), project("api")],
        },
        Instant::now(),
    );
    app
}

/// The rows from the first project heading to the last card.
fn list_rows(app: &App, now: Instant) -> Vec<String> {
    screen(app, now)
        .into_iter()
        .skip(1)
        .take_while(|row| !row.starts_with('└'))
        .map(|row| {
            row.trim_start_matches('│')
                .trim_end_matches('│')
                .trim_end()
                .to_owned()
        })
        .filter(|row| !row.is_empty())
        .collect()
}

#[test]
fn the_list_renders_the_recorded_feed_grouped_by_project() {
    let mut app = app_on_projects();
    let now = Instant::now();

    for batch in recorded_feed() {
        app.apply_changes(&batch)
            .expect("the recorded feed applies");
    }

    assert_eq!(
        list_rows(&app, now),
        [
            "api",
            "> s00000000000b  fake  running",
            "  s00000000000d  other  running",
            "web",
            "  s00000000000a  fake  asleep",
        ]
    );
}

#[test]
fn a_snapshot_replaces_every_session_the_feed_carried_before() {
    let mut app = app_on_projects();
    let feed = recorded_feed();
    app.apply_changes(&feed[0]).unwrap();
    let mut snapshot = feed[0].clone();
    snapshot
        .changes
        .retain(|event| event.entity.id != "s00000000000b");

    app.apply_changes(&snapshot).unwrap();

    let listed: Vec<&str> = app
        .board()
        .listed_sessions()
        .iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(listed, ["s00000000000a", "s00000000000c"]);
}

#[test]
fn a_batch_with_a_session_the_tui_cannot_read_is_refused_whole() {
    let mut app = app_on_projects();
    let feed = recorded_feed();
    app.apply_changes(&feed[0]).unwrap();
    let mut broken: ChangeBatch = feed[1].clone();
    broken.follows = FeedPosition::Cursor(feed[0].cursor.clone());
    let mut event = broken.changes[0].clone();
    event.entity.id = "s00000000000e".to_owned();
    event.change = serde_json::from_value(serde_json::json!({
        "kind": "upsert", "state": {"id": "s00000000000e"}
    }))
    .unwrap();
    broken.changes.push(event);

    let error = app.apply_changes(&broken).expect_err("refused");

    assert!(error.entity.contains("s00000000000e"), "{error}");
    assert!(
        !app.board().listed_sessions().is_empty()
            && app
                .board()
                .listed_sessions()
                .iter()
                .all(|session| session.state == cockpit_protocol::SessionStateView::Running),
        "nothing of the refused batch was applied"
    );
}

#[test]
fn a_session_whose_checkout_is_unknown_is_listed_last_under_no_known_project() {
    let mut app = app_on_projects();
    app.apply_changes(&recorded_feed()[0]).unwrap();

    let unknown_checkout = session("s0000000000ff", 99, 2);
    app.apply_changes(&ChangeBatch {
        follows: FeedPosition::Cursor(recorded_feed()[0].cursor.clone()),
        cursor: recorded_feed()[1].cursor.clone(),
        changes: vec![upsert(
            EntityKind::Session,
            &unknown_checkout.id,
            &unknown_checkout,
        )],
    })
    .unwrap();

    let rows = list_rows(&app, Instant::now());
    assert_eq!(
        rows[rows.len() - 2..],
        ["(no known project)", "  s0000000000ff  fake  running"]
    );
}

#[test]
fn a_session_waiting_on_an_approval_shows_waiting_on_you_and_the_list_still_answers_keys() {
    let mut app = app_on_projects();
    let now = Instant::now();
    for batch in recorded_feed() {
        app.apply_changes(&batch).unwrap();
    }
    queue(
        &mut app,
        vec![run(7, "s00000000000d", "make test")],
        Vec::new(),
        now,
    );
    // Back to the list, then open a session: the answer has not come yet.
    app.key(Key::Tab, now);
    let open = app.key(Key::Enter, now);
    assert_eq!(open.send.len(), 1);

    let moved = app.key(Key::Char('j'), now);

    assert_eq!(moved.used, Some("move"));
    assert_eq!(app.selected(), Some("s00000000000d"));
    assert!(
        screen_text(&app, now).contains("> s00000000000d  other  running  waiting on you"),
        "{}",
        screen_text(&app, now)
    );
}
