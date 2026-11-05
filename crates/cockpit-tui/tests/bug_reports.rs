//! Bug report drafts in the TUI (#471): one needs-you card counts the drafts the feed carries,
//! `B` lists them, and `b` drafts a new one with `bug-report-new`. Review and send stay in the
//! CLI; a draft a send opened is listed apart, and `B` records its issue in place (#847).

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
        opened_at: None,
        issue: None,
    }
}

/// `draft` once `torlek bug-report send` handed the owner its issue, as the feed carries it.
fn opened(draft: BugReportView) -> BugReportView {
    BugReportView {
        opened_at: Some(2),
        changed_at: 2,
        ..draft
    }
}

const ISSUE_URL: &str = "https://github.com/Androlax2/agent-cockpit/issues/844";

/// `draft` as `bug-report-sent` answers it.
fn recorded(draft: BugReportView) -> Reply {
    Reply::BugReport {
        report: Box::new(BugReportView {
            status: BugStatusView::Sent,
            issue_url: Some(ISSUE_URL.to_owned()),
            ..draft
        }),
    }
}

/// An app whose feed carries `drafts`, with the `B` view open.
fn listing(drafts: &[BugReportView], now: Instant) -> cockpit_tui::App {
    let mut app = app_with(&["app"], Vec::new(), now);
    app.apply_changes(&after_snapshot(drafts.iter().map(published).collect()))
        .unwrap();
    press(&mut app, &[Key::Char('B')], now);
    app
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
    assert!(screen.contains("torlek bug-report show ID"), "{screen}");
    assert!(screen.contains("torlek bug-report send ID"), "{screen}");
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
        since: None,
        cli_version: None,
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
            .any(|entry| entry.contains("torlek bug-report show 7")),
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

#[test]
fn the_no_project_hint_stays_beside_the_drafts_card() {
    let now = Instant::now();
    let mut app = app_with(&[], Vec::new(), now);
    let only = draft(1, BugOriginView::Daemon, "the daemon crashed", 1);
    app.apply_changes(&after_snapshot(vec![published(&only)]))
        .unwrap();

    let screen = screen_text(&app, now);

    assert!(screen.contains("Bug report drafts (1)"), "{screen}");
    assert!(screen.contains("No project yet"), "{screen}");
}

#[test]
fn a_draft_on_its_way_takes_no_edit() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);
    press(&mut app, &[Key::Char('b')], now);
    typing(&mut app, "the list froze", now);
    press(&mut app, &[Key::Enter], now);

    typing(&mut app, " again", now);

    let screen = screen_text(&app, now);
    assert!(!screen.contains("froze again"), "{screen}");
    assert!(screen.contains("Drafting…"), "{screen}");
}

/// #847: the owner submitted draft 1 in the browser `torlek bug-report send` opened, and `B`
/// kept listing it as waiting.
#[test]
fn a_draft_a_send_opened_is_listed_as_opened_not_as_waiting() {
    let now = Instant::now();
    let app = listing(
        &[
            opened(draft(1, BugOriginView::Owner, "the board froze", 1)),
            draft(2, BugOriginView::Agent, "todo add timed out", 1),
        ],
        now,
    );

    let screen = screen_text(&app, now);
    let (before_waiting, waiting) = screen
        .split_once("Waiting for you")
        .unwrap_or_else(|| panic!("no waiting drafts:\n{screen}"));
    assert!(
        before_waiting.contains("Opened to send, issue not recorded yet"),
        "{screen}"
    );
    assert!(before_waiting.contains("the board froze"), "{screen}");
    assert!(!waiting.contains("the board froze"), "{screen}");
    assert!(waiting.contains("todo add timed out"), "{screen}");
}

#[test]
fn the_card_counts_the_opened_drafts_apart_from_the_waiting_ones() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);

    app.apply_changes(&after_snapshot(vec![
        published(&opened(draft(
            1,
            BugOriginView::Owner,
            "the board froze",
            1,
        ))),
        published(&draft(2, BugOriginView::Agent, "todo add timed out", 1)),
    ]))
    .unwrap();

    let screen = screen_text(&app, now);
    assert!(
        screen.contains("! Bug report drafts (1), 1 more opened to send: B lists them"),
        "{screen}"
    );
}

#[test]
fn the_card_of_opened_drafts_alone_says_b_records_their_issue() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);

    app.apply_changes(&after_snapshot(vec![published(&opened(draft(
        1,
        BugOriginView::Owner,
        "the board froze",
        1,
    )))]))
    .unwrap();

    let screen = screen_text(&app, now);
    assert!(
        screen.contains("! Bug reports opened to send (1): B records their issue"),
        "{screen}"
    );
}

#[test]
fn enter_then_the_issue_records_it_for_the_selected_draft_with_bug_report_sent() {
    let now = Instant::now();
    let mut app = listing(
        &[opened(draft(1, BugOriginView::Owner, "the board froze", 1))],
        now,
    );
    press(&mut app, &[Key::Enter], now);
    typing(&mut app, ISSUE_URL, now);

    let submitted = sent(&press(&mut app, &[Key::Enter], now));

    assert_eq!(
        submitted,
        vec![Command::BugReportSent {
            report: 1,
            issue_url: Some(ISSUE_URL.to_owned()),
        }]
    );
}

/// #847's acceptance: once the URL is recorded the draft leaves the list and its issue shows.
#[test]
fn a_recorded_draft_leaves_the_list_and_shows_its_issue() {
    let now = Instant::now();
    let submitted_in_browser = opened(draft(1, BugOriginView::Owner, "the board froze", 1));
    let mut app = listing(std::slice::from_ref(&submitted_in_browser), now);
    press(&mut app, &[Key::Enter], now);
    typing(&mut app, "844", now);
    let submitted = sent(&press(&mut app, &[Key::Enter], now));

    app.answer(&submitted[0], Ok(recorded(submitted_in_browser)), now);
    app.apply_changes(&after_snapshot(vec![tombstone(EntityKind::BugReport, "1")]))
        .unwrap();

    let screen = screen_text(&app, now);
    assert!(!screen.contains("the board froze"), "{screen}");
    assert!(
        screen.contains(&format!("Done: bug report 1 is recorded as {ISSUE_URL}")),
        "{screen}"
    );
    assert!(
        app.log()
            .any(|entry| entry == format!("bug report 1 is recorded as {ISSUE_URL}")),
        "{:?}",
        app.log().collect::<Vec<_>>()
    );
}

#[test]
fn j_moves_to_the_next_draft_and_enter_records_that_one() {
    let now = Instant::now();
    let mut app = listing(
        &[
            opened(draft(1, BugOriginView::Owner, "the board froze", 1)),
            opened(draft(2, BugOriginView::Agent, "todo add timed out", 1)),
        ],
        now,
    );
    press(&mut app, &[Key::Char('j'), Key::Enter], now);
    typing(&mut app, "845", now);

    let submitted = sent(&press(&mut app, &[Key::Enter], now));

    assert_eq!(
        submitted,
        vec![Command::BugReportSent {
            report: 2,
            issue_url: Some("845".to_owned()),
        }]
    );
}

#[test]
fn an_issue_left_empty_is_not_recorded() {
    let now = Instant::now();
    let mut app = listing(
        &[opened(draft(1, BugOriginView::Owner, "the board froze", 1))],
        now,
    );

    let submitted = sent(&press(&mut app, &[Key::Enter, Key::Enter], now));

    assert!(submitted.is_empty());
    assert!(
        screen_text(&app, now).contains("Cannot record: give the issue's URL or its number"),
        "{}",
        screen_text(&app, now)
    );
}

#[test]
fn a_refused_record_keeps_what_was_typed_with_why() {
    let now = Instant::now();
    let mut app = listing(
        &[opened(draft(1, BugOriginView::Owner, "the board froze", 1))],
        now,
    );
    press(&mut app, &[Key::Enter], now);
    typing(&mut app, "issue 844", now);
    let submitted = sent(&press(&mut app, &[Key::Enter], now));

    app.answer(
        &submitted[0],
        Err("invalid-request: issue must be an https:// URL or an issue number".to_owned()),
        now,
    );

    let screen = screen_text(&app, now);
    assert!(
        screen.contains("its URL or its number: issue 844"),
        "{screen}"
    );
    assert!(
        screen.contains(
            "Cannot record: invalid-request: issue must be an https:// URL or an issue number"
        ),
        "{screen}"
    );
}

#[test]
fn an_issue_on_its_way_is_not_sent_twice() {
    let now = Instant::now();
    let mut app = listing(
        &[opened(draft(1, BugOriginView::Owner, "the board froze", 1))],
        now,
    );
    press(&mut app, &[Key::Enter], now);
    typing(&mut app, "844", now);

    let submitted = sent(&press(&mut app, &[Key::Enter, Key::Enter], now));

    assert_eq!(submitted.len(), 1);
    assert!(
        screen_text(&app, now).contains("Recording…"),
        "{}",
        screen_text(&app, now)
    );
}

/// `q` closes the list, and is a letter like any other in the issue typed.
#[test]
fn q_typed_into_the_issue_is_text() {
    let now = Instant::now();
    let mut app = listing(
        &[opened(draft(1, BugOriginView::Owner, "the board froze", 1))],
        now,
    );
    press(&mut app, &[Key::Enter], now);

    typing(&mut app, "q", now);

    assert!(
        screen_text(&app, now).contains("its URL or its number: q"),
        "{}",
        screen_text(&app, now)
    );
}

#[test]
fn esc_leaves_the_issue_and_goes_back_to_the_list() {
    let now = Instant::now();
    let mut app = listing(
        &[opened(draft(1, BugOriginView::Owner, "the board froze", 1))],
        now,
    );
    press(&mut app, &[Key::Enter], now);
    typing(&mut app, "844", now);

    press(&mut app, &[Key::Esc], now);

    let screen = screen_text(&app, now);
    assert!(!screen.contains("its URL or its number"), "{screen}");
    assert!(screen.contains("the board froze"), "{screen}");
}

#[test]
fn enter_without_a_draft_asks_for_no_issue() {
    let now = Instant::now();
    let mut app = listing(&[], now);

    press(&mut app, &[Key::Enter], now);

    let screen = screen_text(&app, now);
    assert!(
        screen.contains("No bug report draft waits for you."),
        "{screen}"
    );
    assert!(!screen.contains("its URL or its number"), "{screen}");
}
