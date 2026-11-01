//! Bug report drafts in the TUI (#471): one needs-you card counts the drafts the feed carries,
//! `B` lists them, and `b` drafts a new one with `bug-report-new`. Review and send stay in the
//! CLI.

mod common;

use std::time::Instant;

use cockpit_protocol::{
    BugBundleView, BugOriginView, BugReportView, BugStatusView, Command, EntityKind, Reply,
};
use cockpit_tui::Key;
use common::{
    after_snapshot, app_with, press, screen_text, sent, snapshot, tombstone, typing, upsert,
};

fn draft(id: i64, origin: BugOriginView, title: &str, occurrences: i64) -> BugReportView {
    BugReportView {
        id,
        origin,
        requested_by_owner: false,
        session: None,
        status: BugStatusView::Draft,
        title: title.to_owned(),
        description: "what happened".to_owned(),
        steps: String::new(),
        fingerprint: "cockpit-0.1.0/stall/session-list".to_owned(),
        environment: "cockpit 0.1.0".to_owned(),
        bundle: BugBundleView::Pending,
        occurrences,
        issue_url: None,
        created_at: 1,
        changed_at: 1,
        issue: None,
    }
}

fn published(draft: &BugReportView) -> cockpit_protocol::ChangeEvent {
    upsert(EntityKind::BugReport, &draft.id.to_string(), draft)
}

#[test]
fn the_drafts_on_the_feed_show_as_one_card_with_their_count() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);

    app.apply_changes(&after_snapshot(vec![
        published(&draft(1, BugOriginView::Daemon, "a request stalled", 1)),
        published(&draft(2, BugOriginView::Agent, "todo add timed out", 1)),
    ]))
    .unwrap();

    let screen = screen_text(&app, now);
    assert_eq!(screen.matches("Bug report drafts").count(), 1, "{screen}");
    assert!(
        screen.contains("! Bug report drafts (2): B lists them"),
        "{screen}"
    );
}

#[test]
fn the_card_goes_once_the_last_draft_is_retracted() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);
    let only = draft(1, BugOriginView::Owner, "the board froze", 1);
    app.apply_changes(&after_snapshot(vec![published(&only)]))
        .unwrap();

    app.apply_changes(&after_snapshot(vec![tombstone(EntityKind::BugReport, "1")]))
        .unwrap();

    assert!(!screen_text(&app, now).contains("Bug report drafts"));
}

#[test]
fn a_snapshot_without_drafts_drops_the_ones_shown() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);
    let only = draft(1, BugOriginView::Owner, "the board froze", 1);
    app.apply_changes(&after_snapshot(vec![published(&only)]))
        .unwrap();

    app.apply_changes(&snapshot(Vec::new())).unwrap();

    assert!(app.board().bug_report_drafts().is_empty());
}

#[test]
fn a_bug_report_change_the_tui_cannot_read_refuses_the_batch() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);

    let refused = app.apply_changes(&after_snapshot(vec![tombstone(
        EntityKind::BugReport,
        "not-a-number",
    )]));

    assert!(refused.is_err());
}

#[test]
fn b_upper_lists_each_draft_and_how_to_review_and_send_it_in_the_cli() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);
    let mut repeated = draft(4, BugOriginView::Agent, "todo add timed out", 3);
    repeated.session = Some("s00000000000a".to_owned());
    app.apply_changes(&after_snapshot(vec![published(&repeated)]))
        .unwrap();

    press(&mut app, &[Key::Char('B')], now);

    let screen = screen_text(&app, now);
    assert!(
        screen.contains("4  agent  todo add timed out  (3 times)  from s00000000000a"),
        "{screen}"
    );
    assert!(screen.contains("cockpit bug-report show ID"), "{screen}");
    assert!(screen.contains("cockpit bug-report send ID"), "{screen}");
}

#[test]
fn b_drafts_a_report_with_bug_report_new_and_closes_once_answered() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);
    press(&mut app, &[Key::Char('b')], now);
    typing(&mut app, " the list froze ", now);
    press(&mut app, &[Key::Tab, Key::Tab], now);
    typing(&mut app, "open F twice", now);

    let submitted = sent(&press(&mut app, &[Key::Enter], now));
    let expected = Command::BugReportNew {
        description: "the list froze".to_owned(),
        title: None,
        steps: "open F twice".to_owned(),
    };
    assert_eq!(submitted, std::slice::from_ref(&expected));
    app.answer(
        &expected,
        Ok(Reply::BugReport {
            report: Box::new(draft(7, BugOriginView::Owner, "the list froze", 1)),
        }),
        now,
    );

    assert!(app.bug_form().is_none());
    assert!(
        app.log()
            .any(|entry| entry.contains("cockpit bug-report show 7")),
        "{:?}",
        app.log().collect::<Vec<_>>()
    );
}

#[test]
fn a_report_without_a_description_is_not_sent() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);
    press(&mut app, &[Key::Char('b')], now);

    let submitted = sent(&press(&mut app, &[Key::Enter], now));

    assert!(submitted.is_empty());
    assert!(screen_text(&app, now).contains("Cannot draft: describe what went wrong"));
}

#[test]
fn a_refused_draft_keeps_the_form_open_with_why() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);
    press(&mut app, &[Key::Char('b')], now);
    typing(&mut app, "the list froze", now);
    let submitted = sent(&press(&mut app, &[Key::Enter], now));

    app.answer(
        &submitted[0],
        Err("internal: the database is busy".to_owned()),
        now,
    );

    assert!(app.bug_form().is_some());
    assert!(
        screen_text(&app, now).contains("Cannot draft: internal: the database is busy"),
        "{}",
        screen_text(&app, now)
    );
}
