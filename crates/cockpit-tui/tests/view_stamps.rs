//! #706: the change feed's view stamps tell the TUI which of the views the daemon answers by
//! request changed, so it asks for those at once instead of on a timer.
mod common;
use std::time::Instant;

use cockpit_protocol::{Command, EntityKind, ViewArea, ViewStamp};
use cockpit_tui::{App, Key};
use common::{after_snapshot, app_with, snapshot, upsert};

fn stamp(area: ViewArea, changed_at: u64) -> cockpit_protocol::ChangeEvent {
    upsert(EntityKind::ViewStamp, area.id(), &ViewStamp { changed_at })
}

fn quiet_app(now: Instant) -> App {
    let mut app = app_with(&["app"], Vec::new(), now);
    app.apply_changes(&snapshot(ViewArea::ALL.map(|area| stamp(area, 1)).to_vec()))
        .unwrap();
    app.changed_view_commands();
    app
}

#[test]
fn a_moved_stamp_asks_for_that_area_only() {
    let mut app = quiet_app(Instant::now());

    app.apply_changes(&after_snapshot(vec![stamp(ViewArea::Projects, 2)]))
        .unwrap();

    assert_eq!(app.changed_view_commands(), [Command::ProjectList]);
}

#[test]
fn each_closed_area_asks_for_its_own_request() {
    let mut app = quiet_app(Instant::now());

    app.apply_changes(&after_snapshot(vec![
        stamp(ViewArea::AgentReports, 2),
        stamp(ViewArea::Approvals, 2),
    ]))
    .unwrap();

    let mut asked = app.changed_view_commands();
    asked.sort_by_key(|command| format!("{command:?}"));
    assert_eq!(
        asked,
        [
            Command::AgentReports,
            Command::ApprovalQueue,
            Command::NetworkList
        ]
    );
}

#[test]
fn the_same_stamp_again_asks_for_nothing() {
    let mut app = quiet_app(Instant::now());

    app.apply_changes(&after_snapshot(vec![stamp(ViewArea::Projects, 1)]))
        .unwrap();

    assert_eq!(app.changed_view_commands(), []);
}

#[test]
fn a_moved_stamp_is_asked_for_once() {
    let mut app = quiet_app(Instant::now());
    app.apply_changes(&after_snapshot(vec![stamp(ViewArea::Projects, 2)]))
        .unwrap();
    app.changed_view_commands();

    assert_eq!(app.changed_view_commands(), []);
}

#[test]
fn the_first_stamps_ask_for_every_area() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);

    app.apply_changes(&snapshot(ViewArea::ALL.map(|area| stamp(area, 1)).to_vec()))
        .unwrap();

    let mut asked = app.changed_view_commands();
    asked.sort_by_key(|command| format!("{command:?}"));
    assert_eq!(
        asked,
        [
            Command::AgentReports,
            Command::ApprovalQueue,
            Command::NetworkList,
            Command::ProjectList
        ]
    );
}

#[test]
fn a_stamp_that_moved_during_an_outage_is_found_after_the_new_snapshot() {
    let mut app = quiet_app(Instant::now());

    app.apply_changes(&snapshot(vec![stamp(ViewArea::Approvals, 9)]))
        .unwrap();

    assert_eq!(
        app.changed_view_commands(),
        [Command::ApprovalQueue, Command::NetworkList]
    );
}

#[test]
fn a_task_stamp_asks_only_while_the_task_view_is_open() {
    let now = Instant::now();
    let mut app = quiet_app(now);
    app.apply_changes(&after_snapshot(vec![stamp(ViewArea::Tasks, 2)]))
        .unwrap();
    assert_eq!(app.changed_view_commands(), []);

    app.key(Key::Char('T'), now);
    app.apply_changes(&after_snapshot(vec![stamp(ViewArea::Tasks, 3)]))
        .unwrap();

    assert_eq!(app.changed_view_commands(), app.view_refresh_commands());
}

#[test]
fn a_workflow_stamp_asks_only_while_the_workflow_view_is_open() {
    let now = Instant::now();
    let mut app = quiet_app(now);
    app.apply_changes(&after_snapshot(vec![stamp(ViewArea::Workflows, 2)]))
        .unwrap();
    assert_eq!(app.changed_view_commands(), []);

    app.key(Key::Char('F'), now);
    app.apply_changes(&after_snapshot(vec![stamp(ViewArea::Workflows, 3)]))
        .unwrap();

    assert_eq!(app.changed_view_commands(), app.view_refresh_commands());
}

#[test]
fn a_stamp_of_an_area_this_version_does_not_know_is_ignored() {
    let mut app = quiet_app(Instant::now());
    let unknown = upsert(
        EntityKind::ViewStamp,
        "gardens",
        &ViewStamp { changed_at: 2 },
    );

    app.apply_changes(&after_snapshot(vec![unknown])).unwrap();

    assert_eq!(app.changed_view_commands(), []);
}

#[test]
fn a_stamp_the_tui_cannot_read_refuses_the_batch() {
    let mut app = quiet_app(Instant::now());
    let broken = upsert(
        EntityKind::ViewStamp,
        "projects",
        &serde_json::json!({"x": 1}),
    );

    let refused = app.apply_changes(&after_snapshot(vec![broken]));

    assert!(refused.is_err());
}
